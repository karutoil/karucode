# Plugins

karucode plugins are **separate processes**. A plugin is an executable that
speaks newline-delimited JSON-RPC 2.0 on stdin/stdout; the harness spawns it,
supervises it and exposes its tools to the model as `plugin__<plugin>__<tool>`.

There is no dynamic loading, no ABI and no link-time coupling: a plugin can be
written in Rust with [`kc-plugin-sdk`](../crates/kc-plugin-sdk), in Python, in Go
or in shell, as long as it speaks protocol version `1`.

* Host: [`crates/kc-plugin-host`](../crates/kc-plugin-host)
* SDK: [`crates/kc-plugin-sdk`](../crates/kc-plugin-sdk)
* Example: [`plugins/example-hello`](../plugins/example-hello)
* Protocol types (shared by host and SDK): [`crates/kc-core/src/plugin.rs`](../crates/kc-core/src/plugin.rs)

---

## 1. Execution model: why out of process

Three plausible designs were considered. This is the honest comparison that led
to the current one; the numbers are order-of-magnitude, measured on a normal
Linux development machine, not benchmark-grade.

| | In-process Rust (trait objects) | WASM (wasmtime) | Out-of-process process (**chosen**) |
| --- | --- | --- | --- |
| Isolation from the harness | **None.** A panic aborts the task at best; a segfault, `exit()` or a leaked thread takes the server down. Bugs are shared memory bugs. | **Strong at the memory level**, and capability-based: a module can only touch what the host links in (no filesystem/network unless provided). | **Process isolation.** A crash, hang or memory leak kills that plugin only. Supervised, restartable, killable. |
| Security model | Whatever the plugin does, the harness does. | Wasmtime's sandbox + explicitly linked imports; a good fit for untrusted third-party code. | The plugin runs **as the server user with full OS permissions** — same trust level as a shell command. Not a sandbox. |
| Startup | Free (already linked). | ~1–10 ms to instantiate a small module after ~10–40 ms of runtime init (much more with a big preopened module set). | ~2–20 ms to fork/exec + Python/Rust interpreter start (a Python plugin is closer to 30–60 ms). One-time; calls are then a pipe round-trip (~0.1–0.5 ms). |
| Memory per plugin | Shares the address space; a plugin can balloon the server RSS. | Linear memory per instance, usually a few MB, plus the shared runtime. | Separate RSS: ~2–4 MB for a small native binary, 10–30 MB for Python/Node. Very visible in `GET /api/stats` as separate processes, invisible in the server's own RSS. |
| Language compatibility | Rust only, must be rebuilt with the same toolchain and crate versions. | Anything that compiles to WASI, with a narrower standard library (no threads, no `fork`, sockets only via host imports, no `nproc`-style syscalls). Existing tools need porting. | **Any language, any runtime, any version.** No rebuild when the harness changes. |
| Failure handling | The process dies with the plugin. | A trap kills the instance; the host stays healthy. | A crash is visible (`failed` status + exit code); the host restarts once per failed call, up to 3 times. |
| Streaming / blocking I/O | Direct calls. | Host imports; blocking calls need async plumbing. | Natural: the plugin is a normal program doing normal blocking I/O; the host is fully async on top of pipes. |
| Debugging | `gdb`/test harness in the server. | Wasm debuggers, decent but unusual. | `strace`, `gdb`, run it by hand on a terminal — the protocol is just lines of JSON. |
| Complexity in the harness | Low. | High: runtime dependency (~5–10 MB binary growth), import surface, fuel/epoch limits. | Medium: spawn, pipe, framing, supervision, backpressure. All contained in `kc-plugin-host`. |

**Chosen: out-of-process.** The product promise is "one Rust binary, no Node at
runtime", so a heavyweight WASM runtime is a poor trade; and an in-process Rust
plugin loader would let a plugin bug take down every user's session. Process
isolation gives the failure containment that matters (a plugin cannot corrupt or
crash the harness) with the compatibility that makes plugins worth writing.

**What is explicitly not claimed:** the plugin is *not* sandboxed. A plugin can
read your files, spawn processes and open sockets, exactly like `run_command`.
The harness treats plugin output as untrusted (it is truncated and rendered as
text), and the configured `permissions` list only decides which tools the host
exposes — the manifest cannot grant itself more than the config entry allows. If
you need real isolation, run karucode inside a container or a VM.

---

## 2. Discovery

Two sources are merged, in this order:

1. **Explicit entries** in `config.toml`:

   ```toml
   [[plugins]]
   name = "example-hello"                 # must match the manifest name
   command = ["target/debug/example-hello"]  # argv[0] is resolved on PATH
   enabled = true
   permissions = ["files.read"]           # declarative, see §6
   env = { RUST_LOG = "debug" }           # extra environment for the process
   settings = { greeting = "hi" }         # free-form, delivered as JSON

   [[plugins]]
   name = "remote-tools"
   command = ["python3", "/opt/karucode/plugins/remote.py"]
   enabled = false                        # discovered, not started
   ```

2. **Manifests on disk**: `<data_dir>/plugins/<name>/plugin.toml`, where
   `data_dir` is `data_dir` from the config (default
   `$XDG_DATA_HOME/karucode` / `~/.local/share/karucode`).

   ```toml
   # ~/.local/share/karucode/plugins/wordy/plugin.toml
   name = "wordy"                    # optional: the directory name is the fallback
   command = ["/opt/plugins/wordy"]  # required
   enabled = true
   permissions = ["files.read", "network"]

   [env]
   WORDY_CACHE = "/tmp/wordy"

   [settings]
   max_words = 500
   ```

Rules:

* Plugin **names must be unique**. A duplicate (config vs. disk, or twice on
  disk) fails the slot with
  `duplicate plugin name `x`: declared by … and again by …`, so you notice
  instead of silently running one of two plugins.
* A missing or empty `command` fails the slot immediately with
  `plugin `x` from <source> has no command; add `command = ["program", "args"]"`.
  Discovery never panics and never blocks startup.
* A malformed `plugin.toml` fails the slot with the file path and the TOML error.
* A plugin with no `name` in `plugin.toml` uses its directory name.
* `enabled = false` means "discovered, not started" — status `disabled`.

### Environment the host sets for the process

| Variable | Meaning |
| --- | --- |
| `KARUCODE_PLUGIN_NAME` | The name the host expects the manifest to report. |
| `KARUCODE_PLUGIN_PROTOCOL` | `1`. |
| `KARUCODE_PLUGIN_PERMISSIONS` | Comma-separated declared permissions (only when non-empty). |
| `KARUCODE_PLUGIN_SETTINGS` | The `settings` table as a JSON object (only when non-empty). |

Anything in `env` is added too (it wins over the inherited environment).

---

## 3. Lifecycle

```
discover ──▶ stopped ──start──▶ starting ──initialize──▶ ready
                 ▲                  │                     │
                 │               (failure)            calls / hooks
                 │                  ▼                     │
                 └─ stop ◀──── disabled               failed ◀─ process exit
```

| Status | Meaning |
| --- | --- |
| `stopped` | Discovered, enabled, process not running (before `start_all`, or after an explicit `shutdown`). |
| `starting` | Process spawned, handshake in flight. |
| `ready` | Handshake done; tools are exposed and hooks are delivered. |
| `failed` | Could not spawn, timed out, reported a bad manifest/protocol, or the process exited. `error` says why, including exit code/signal. |
| `disabled` | `enabled = false` or disabled at runtime. No process, no tools. |

Startup (`PluginHost::start_all`) spawns every enabled plugin **concurrently**
and runs the handshake per plugin. One broken plugin cannot delay or break the
others.

Host shutdown sends `shutdown`, waits briefly, then kills and reaps the process; a
plugin that ignores `shutdown` is still killed. Reload, removal and host shutdown
permanently retire that slot generation: a tool handle retained by an in-flight run
fails without executing or spawning the old code again. Dropping a live
`PluginHost` synchronously signals each process group and leaves its retained
supervisor to reap the child. Ordinary enable/disable is different: disabling
stops the current generation without retiring it, and enabling starts it again.

### Restart policy

If a tool call finds the process dead, the host makes **one** restart attempt
(spawn + handshake), up to **3 restarts per plugin per server lifetime**. Beyond
that the call fails with
`plugin `x` is not running (…) and has already been restarted 3 times; fix the
plugin or reload it`. Enabling or reloading the plugin resets the budget. The
`restart_count` field in `GET /api/plugins` shows the current count.

### Installing, upgrading and removing without a restart

The host re-reads its sources on demand, so none of these needs a server restart:

| You want to… | Do this | Then |
| --- | --- | --- |
| install a plugin | write `<data_dir>/plugins/<name>/plugin.toml` (or add a `[[plugins]]` entry) | `POST /api/plugins/reload` |
| upgrade one | rebuild the binary in place, or edit its manifest | `POST /api/plugins/:name/reload` |
| remove one | delete its directory / `[[plugins]]` entry | `POST /api/plugins/reload` |
| none of the above | — | leave it to `plugins_watch` |

```bash
curl -s -X POST localhost:8787/api/plugins/reload \
  -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' -d '{}' | jq .report
# { "added": ["wordy"], "removed": ["old"], "restarted": [], "unchanged": ["example-hello"] }
```

`PluginHost::rescan` diffs the entries it discovers against the slots it is
running and does exactly one of four things per name:

* **add** — new name: build a slot and start it (hot load);
* **remove** — name gone from every source: send `shutdown`, kill, drop its tools
  (hot unload);
* **restart** — same name but a different config entry, a different manifest, or a
  different **file** (hot reload);
* **leave alone** — everything identical. A rescan of an unchanged tree does not
touch a running process, so the watcher cannot churn a plugin's in-memory state
by polling.

File changes are detected by fingerprinting every command argument that resolves
to a file (`mtime` + length) plus the manifest, which is what makes "recompile,
then reload" work even though the command line is byte-identical. Replacing a
**script** in a `command = ["python3", "plugin.py"]` plugin counts too: both
arguments are fingerprinted.

`enabled` is a runtime toggle once a slot exists, so a rescan never silently
re-enables a plugin the operator switched off. The config file decides what a
*new* plugin starts as.

Both routes rebuild the runtime's tool registry before they answer, so the model
sees the new tools on its next call. A run already in flight keeps the registry it
started with: its old tool handle is permanently retired and fails with
`generation was permanently retired` rather than hanging or
respawning replaced code. The next run sees the fresh registry. Long-lived hook and
panel handles are different again: they dynamically follow the host's current slot
membership across reload/rescan, while filtering each generation's readiness.

**Automatic reloads.** To skip the curl entirely:

```toml
# ~/.config/karucode/config.toml
plugins_watch = true      # or KARUCODE_PLUGINS_WATCH=1
plugins_watch_secs = 2    # or KARUCODE_PLUGINS_WATCH_SECS; clamped to 1..=600
```

It **polls** — one `read_dir` plus a `stat` per configured command per tick —
rather than using inotify: no extra dependency, works on network filesystems and
container bind mounts, and a write that lands while the server is busy is still
applied on the next tick. Off by default, because a reload replaces processes and
that should be an explicit choice. The stamp it compares also covers the config
file, so editing `[[plugins]]` there is picked up as well.

---

## 4. Protocol v1 (wire spec)

**Framing:** one JSON object per line (`\n`-terminated) in both directions.
stdout is reserved for the protocol; **write logs to stderr** (the host drains
them into its own log at `debug`). A non-JSON line on stdout is logged and
ignored; it is not fatal.

**Encoding:** UTF-8 JSON, JSON-RPC 2.0 envelope:

```json
{"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "hello", "arguments": {"name": "Ada"}}}
```

Responses carry the same `id` and either `result` or `error`:

```json
{"jsonrpc": "2.0", "id": 7, "result": {"ok": true, "content": "Hello, Ada!", "meta": null}}
{"jsonrpc": "2.0", "id": 7, "error": {"code": -32601, "message": "unknown tool `nope`"}}
```

Notifications have **no `id`** and get no response. Everything the host sends is
a request (it waits for the response); everything it sends can be answered in any
order — the host routes by `id` and never blocks the process on one outstanding
call at a time.

### 4.1 `initialize` (host → plugin)

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol":"1"}}
```

Result: the **manifest**:

```json
{
  "name": "example-hello",
  "version": "0.1.0",
  "description": "Example plugin",
  "protocol": "1",
  "capabilities": ["tools"],
  "permissions": ["files.read"],
  "hooks": ["before_prompt"],
  "tool_prefix": null,
  "panels": [
    {"id": "todo", "title": "Todo", "icon": "check", "source": "static",
     "tool": null, "refresh_ms": 0}
  ]
}
```

* `name` must equal the name the host was configured with, otherwise the plugin
  fails with an error naming both.
* `protocol` must be `"1"`.
* `hooks` lists the hooks the plugin wants (`hooks/run` is only sent for these).
* `capabilities` and `permissions` are informational/declarative; the effective
  permission set is computed from the config entry (§6, §8).
* `panels` is optional and **every field in it has a default**, so a manifest
  that declares none (or omits the key entirely) behaves exactly as before —
  this is additive and did not bump the protocol version. See §4.8 and §8.
* `ui` is optional in the same way: declarative buttons, palette commands and
  hide requests, each field defaulted. See §4.9 and §8.
* Budget: **10 s** (`PluginTimeouts::handshake`).

### 4.2 `tools/list` (host → plugin)

Sent right after `initialize`.

```json
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
```

```json
{"jsonrpc":"2.0","id":2,"result":{"tools":[
  {"name":"hello","description":"Greet someone.","parameters":{"type":"object"},"class":null}
]}}
```

* `name` is required, non-empty, and may not contain whitespace. The host
  prefixes it (`plugin__<plugin>__<tool>`).
* `parameters` is a JSON Schema for the arguments object. Default:
  `{"type":"object","properties":{}}`.
* `class` is optional: `read_only` | `mutating` | `shell` | `external`
  (default `external`, which goes through the runtime approval policy). See §6.
* Budget: the same 10 s handshake budget.

### 4.3 `tools/call` (host → plugin)

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"hello","arguments":{"name":"Ada"}}}
```

Result: a [`PluginToolResult`](../crates/kc-core/src/plugin.rs):

```json
{"ok": true, "content": "Hello, Ada!", "meta": {"chars": 10}}
```

* `ok: false` is a **tool failure** (shown to the model, plugin stays connected).
* A JSON-RPC `error` is a **protocol-level failure** (`-32601` for an unknown
  tool is the conventional answer). Both become failed tool outcomes.
* Timeout: **60 s** (`PluginTimeouts::call`); cancellation of the run abandons
  the wait and the result is discarded.
* Arguments are passed through untouched.
* Since this revision the params also carry a `context` object, so a
  workspace-scoped plugin knows which project the call belongs to instead of
  remembering the most recent `before_prompt` (which cross-talks as soon as two
  sessions run concurrently):

  ```json
  {"jsonrpc":"2.0","id":3,"method":"tools/call",
   "params":{"name":"todo_list","arguments":{},
             "context":{"session_id":"s_1","run_id":"r_1","agent_id":null,
                        "workspace":"/home/me/project","project":null,"depth":0}}}
  ```

  This is additive: a plugin that ignores `params.context` behaves exactly as
  before, and the SDK's `ToolRequest::context()` returns `None` on an older host.
  `project` is `null` for a tool call because the run's workspace is already
  canonically resolved; `GET /api/plugins/:name/panels/:panel?project=` does
  populate it.

### 4.4 `hooks/run` (host → plugin)

```json
{"jsonrpc":"2.0","id":4,"method":"hooks/run","params":{"hook":"before_prompt","payload":{"session_id":"s_1","workspace":"/w","turn":"where is the journal written?"}}}
```

```json
{"jsonrpc":"2.0","id":4,"result":{"output":"Plugin note: …"}}
```

`output` may be a string, an object with a `text` field, or any JSON value; a
`null`/missing output means "no contribution". Timeout: **5 s**; failures are
logged and ignored. See §7.

For `before_prompt` the payload also carries `turn`: the text of the user message
the run is answering, so a plugin can retrieve against the actual question instead
of memoising one block per session. It is capped at 4 KiB (cut with the usual
`[… truncated]` marker) and the field is **absent** when the run was started
without a user turn, such as a continuation. Ignoring it is fully backwards
compatible.

### 4.5 `shutdown` (host → plugin)

```json
{"jsonrpc":"2.0","id":5,"method":"shutdown","params":{}}
```

Answer with `{"jsonrpc":"2.0","id":5,"result":null}` and exit 0 if you can. The
host kills the process either way (2 s grace).

### 4.6 `log` (plugin → host, notification)

Optional, for structured logging instead of raw stderr:

```json
{"jsonrpc":"2.0","method":"log","params":{"level":"debug","message":"loaded 12 words"}}
```

The host logs it at debug level. Unknown notifications are ignored; messages
without `id` never get a response.

### 4.7 `notify` (plugin → host, notification)

Ask a human to look at something. This is the **only** way a plugin can reach a
person: the host serves no plugin routes and runs no plugin code in the app's
origin, so there is no other channel.

```json
{"jsonrpc":"2.0","method":"notify",
 "params":{"kind":"needs_input","title":"Deploy needs a yes",
           "body":"staging is approved, production is not",
           "sticky":true}}
```

| Field | Required | Meaning |
| --- | --- | --- |
| `kind` | yes | `run_finished`, `needs_input`, `error` or `info`. Anything else is dropped. |
| `title` | yes | One line, capped at 120 characters. An empty title drops the frame. |
| `body` | no | Capped at 400 characters. |
| `session_id` | no | Which session it is about. Omit and the host uses the session it called you for. |
| `sound` | no | A cue name: `finished`, `attention`, `error`, `tick`. An unknown name uses the kind's default. |
| `sticky` | no | Keep the desktop notification on screen until dismissed. |

The frame is a notification (no `id`), so nothing is sent back. Which is fine —
the host validates it defensively because the input is another process:

* an unknown `kind`, an empty `title`, non-string values or params that are not an
  object are logged at debug and **dropped**;
* text is truncated, so a plugin cannot push a 60 KB tool result into a desktop
  notification;
* a plugin raising more than one notification per second (burst of five) has the
  extra ones dropped with one warning — a plugin in a loop must not be able to
  flood a desktop;
* a malformed frame never fails the host, never fails a run, and never disturbs
  the plugin: it keeps answering tool calls.

Accepted notices become the same `notification` event the runtime emits, with
`source` set to the plugin's name, so the UI can say where it came from. That is
why a plugin cannot spoof a runtime notification.

With the SDK:

```rust
use sdk::{notify, NotifyKind};

notify(NotifyKind::NeedsInput, "Deploy needs a yes", "staging is approved")?;
```

`notify` writes to stdout, which is the protocol channel — log with `eprintln!`,
never `println!`. For the optional fields there is `NotifyFrame` + `notify_with`.

A plugin's `[notifications] plugin = false` refuses all of this at the host, so a
plugin that keeps sending gets nothing through. See the `notifications` topic.


### 4.8 `panels/read` (host → plugin)

The host asks for the payload of a panel the manifest declared:

```json
{"jsonrpc":"2.0","id":12,"method":"panels/read",
 "params":{"panel":"todo","session_id":"s_1","project":"p_1","workspace":"/w"}}
```

```json
{"jsonrpc":"2.0","id":12,"result":{"title":"Todo","data":{"items":[]}}}
```

* `data` is **arbitrary JSON** and the host never interprets it: it is forwarded
  to the browser verbatim and rendered with a component registered for that panel
  type, or as read-only JSON. A plugin therefore cannot return markup, a URL to
  navigate to, or script — which is what keeps the app's CSP and
  `frame-ancestors 'none'` meaningful. Do not try to sneak HTML in; it is shown
  as text.
* `title` is optional; the manifest's `title` is used when it is missing.
* `session_id`, `project` and `workspace` describe the scope the browser asked
  about. `session_id` is `""` and `project` is `null` when the caller named none.
* An unknown `panel` is `-32602` ("invalid params"). Any other error is shown to
  the user next to the tab.
* Budget: the **60 s** call timeout (`PluginTimeouts::call`).
* **Fallback.** A plugin built before this method existed answers `-32601`
  ("method not found"). The host then calls the panel's `tool` through
  `tools/call` and uses its `meta` as the payload — that is what
  `source = "tool"` documented before `panels/read`, so an old plugin keeps
  working.

---

### 4.9 `ui/action` (host → plugin)

The host runs an action a **person activated** — a contributed button or a
command-palette entry:

```json
{"jsonrpc":"2.0","id":13,"method":"ui/action",
 "params":{"action":"run-check",
           "contribution":{"kind":"button","id":"check","label":"Run check",
                           "slot":"composer.toolbar"},
           "session_id":"s_1","project":"p_1","workspace":"/w"}}
```

```json
{"jsonrpc":"2.0","id":13,
 "result":{"ok":true,"message":"Checked 3 files","refresh":["todo"],"open_url":null}}
```

* `action` is the **action id** from the manifest, not the contribution id: two
  contributions may share one handler. An action no contribution declares is
  `-32601`, so a stale page cannot make the plugin run something it no longer
  offers.
* `message` is a toast a person reads. It is capped at 2 000 bytes by the host.
* `refresh` names this plugin's **panels** to re-read afterwards (bare ids, at
  most 16, non-panel-id entries dropped).
* `open_url` opens a tab, and only an absolute `http`/`https` URL survives; the
  browser re-checks it before calling `window.open`.
* `ok: false` renders the message as an error. Omitting `ok` means success, so
  `{"message": "done"}` is enough.
* Budget: the **60 s** call timeout. A click that takes that long is a click
  nobody makes twice — use `notify` or a tool for long work.

This is **not** a tool call. It runs because somebody clicked a control the
plugin itself contributed, so it does not go through the approval policy (the
click is the approval), it never reaches the model, and it cannot change the tool
registry. It is also the only way a panel button can make the plugin *do*
something — before this method, the only option was to post a user turn.

---

### 4.10 Error codes

| Code | Meaning |
| --- | --- |
| `-32700` | Parse error (the SDK answers this for a malformed line). |
| `-32601` | Method or tool not found. |
| `-32602` | Invalid params. |
| `-32000…-32099` | Plugin-defined failures. |

The host never sends `-32600`/`-32602` itself; the text of your `message` is
surfaced in the tool failure, so write something actionable.

## 5. Tool exposure

* Name: `plugin__<plugin>__<tool>`. The prefix makes collisions with first-party
  tools impossible.
* Description: your `description`, trimmed, or `Plugin tool \`<tool>\`.` when it
  is empty. The host does **not** append an origin/trust/permission paragraph to
  every tool. Instead the system prompt names the plugins whose tools are exposed
  (sorted and deduplicated) and states once that plugin tools run out of process
  under host supervision, return untrusted output and follow the approval policy.
  A plugin with no exposed tools gets no prompt note. Version and manifest
  metadata remain visible in `GET /api/plugins`; the config-authoritative
  permissions in §6 still gate which tools are exposed. Removing the per-tool
  copies keeps a 55-tool plugin set from spending thousands of tokens repeating
  the same paragraph in the request's `tools` JSON on every model call.
* Parameters: your `parameters` schema, unchanged.
* Class: the plugin's `class` declaration is only a **permission-gating hint**;
  every exposed plugin tool is actually `external`. A plugin therefore cannot
  skip approval by declaring itself `read_only`. Only the operator's
  `[[plugins]]` entry can name specific bare or namespaced tools in
  `settings.read_only_tools = [...]` to downgrade them to `read_only`. In
  `ask`/`ask_commands` external tools prompt, in `auto` they are allowed and
  audited, and in `read_only` mode external/mutating/shell tools are denied. An
  operator-downgraded read-only tool is auto-allowed. (An MCP server's
  `require_approval = false` is the analogous explicit exception; `read_only`
  still denies it — see [`MCP.md` §4](./MCP.md#4-approval-behaviour).)
* Every failure — dead process, timeout, malformed response, remote error — is a
  failed `ToolOutcome` with text that says what happened and what to try. A
  broken plugin can never panic the host or corrupt the tool registry.

---

## 6. Permissions (declarative, not a sandbox)

`permissions` is a list of strings in the manifest and/or the config entry.
Reserved values: `files.read`, `files.write`, `shell`, `network`,
`session.read` (free-form strings are allowed).

**The host configuration is authoritative and fail-closed.** Effective
permissions are:

* exactly the explicit narrow `PluginConfig.permissions` names (`files.read`,
  `shell`, …); an empty list grants **none**, with no fallback to the runtime
  manifest's `permissions`;
* when the operator's own `[[plugins]]` entry in `config.toml` uses `permissions =
  ["*"]` or `settings.trust = true`, the runtime manifest declaration instead;
* the wildcard and both approval-exemption settings (`trust`, `read_only_tools`)
  are ignored in a plugin-shipped disk `plugin.toml`, with a warning in diagnostics.
  Narrow names in that disk manifest remain deliberate reviewed grants, but they
  cannot delegate whatever the running process later chooses to declare.

Runtime-manifest permissions outside the effective set are ignored and reported
in `GET /api/plugins` (in the plugin's `error` field, with status still `ready`)
and logged. The ignored-wildcard warning is also retained there.

Enforcement happens **at the tool-exposure layer**: a tool whose declared class
needs a permission that is not in the effective set is **not exposed** at all
(the reason is logged at warn level):

| Tool class | Required permission | If missing |
| --- | --- | --- |
| `read_only` | — | exposed |
| `external` | — | exposed (goes through the runtime approval policy) |
| `mutating` | `files.write` | hidden |
| `shell` | `shell` | hidden |

**This is not an OS sandbox.** The plugin process runs with the server's user and
can do anything that user can do, whatever it declares. The list exists so a
plugin's tool surface is an explicit, reviewable decision rather than a surprise;
it stops accidental over-exposure, not malicious code. Treat any plugin you
install as you would a shell script you run as your own user.

---

## 7. Hooks

`call_hook(hook, payload)` fans out to every **ready** plugin that advertises the
hook in its manifest, in parallel, with a 5 s budget each, and returns the
non-null `output` values. A failing hook never fails a run.

| Hook | Status | Payload | Output |
| --- | --- | --- | --- |
| `before_prompt` | **implemented** | `{session_id, workspace, turn}` | text appended to the system prompt |
| `on_run_end` | **implemented** | `{session_id, run_id, status}` | ignored |
| `on_tool_result` | **implemented** | `{session_id, tool, ok}` | ignored |
| `on_notify` | **implemented** | `{session_id, run_id, kind, title, body, source}` | ignored |
| `on_run_start`, `on_tool_call`, `on_model_request`, `on_model_response`, `on_session_create` | reserved | — | — |

`on_notify` is how a plugin **observes** notifications: it is called for every
`notification` the runtime emits (a run finishing, an approval appearing, a run
failing), which is what lets a plugin bridge karucode to ntfy, Pushover, a webhook
or a desktop daemon without karucode growing that integration. The other direction —
a plugin *raising* a notification — is the `notify` frame in §4.7.

The call is fire-and-forget: the runtime spawns it after the run has already ended,
so a slow plugin cannot delay a run, and it never fails one. Advertise it with the
SDK:

```rust
use sdk::{PluginBuilder};

PluginBuilder::new("bridge", "0.1.0")
    .on_notify(|payload| {
        // payload: {session_id, run_id, kind, title, body, source}
        eprintln!("karucode: {} {}", payload["kind"], payload["title"]);
        serde_json::json!({})
    })
    .run()
```

Reserved hooks are never called: advertising one is harmless, it just does
nothing today. `kc_core::plugin::hooks::IMPLEMENTED` is the authoritative list.

`PluginHost` implements `kc_core::hooks::Hooks`. It also offers a cheap, live
`hooks_handle()` so the server can keep the host (for `set_enabled` / `shutdown`,
which need `&mut self`) while handing an `Arc<dyn Hooks>` to the runtime. The
handle shares both per-plugin state and dynamic slot membership: enable/disable is
immediate, and a handle created before `reload`/`rescan` follows the new generation
instead of going stale. `panels_handle()` has the same semantics. Calls snapshot
only the current Arc list, then drop the membership mutex before plugin I/O. Old
tool objects deliberately do not follow replacements; their generation is retired.

```rust
let mut host = kc_plugin_host::PluginHost::discover(&config.plugins, &config.data_dir);
host.set_timeouts(kc_plugin_host::PluginTimeouts::default()); // optional
host.start_all().await;

let tools = host.tools();                                    // feed into ToolRegistry
let hooks: Arc<dyn kc_core::hooks::Hooks> = Arc::new(host.hooks_handle());
// keep `host` behind a lock for GET /api/plugins and set_enabled(name, enabled)
```

`tools()` is a live snapshot: after enabling or disabling a plugin, call it again
to pick up the new registry (the returned tools are cheap `Arc<dyn Tool>` values).

---

## 8. Capabilities

`capabilities` is informational metadata for the UI (`["tools"]`,
`["tools", "hooks"]`, `["tools", "panels"]`, …). The host does not gate
behaviour on it today; it exists so a future capability negotiation can be
additive. The one convention worth keeping: declare `panels` when the manifest
declares any panel (the SDK's `.panel(…)` does this for you), because the
settings screen uses it to explain why a plugin has a tab.

### Panels and the UI

A panel is a tab in the web UI's inspector. The plugin supplies a `data` payload
over `panels/read` (§4.8); the *frontend* decides how to draw it. There is no
route mounting, no asset serving and no script execution for plugins, so a panel
cannot add behaviour to the app — only content that a first-party renderer (or
the generic JSON view) understands.

Validation happens at the handshake, so a broken declaration is a readable
`PluginDiagnostics.error` rather than a tab that fails every time it is clicked:

| Declaration | Result |
| --- | --- |
| id with anything but `a-z0-9-_.` | handshake error naming the id |
| the same `id` twice | handshake error ("declared twice") |
| `source` other than `tool`/`static` | handshake error naming the source |
| `source = "tool"` naming an unadvertised tool | handshake error naming the tool and `tools/list` |
| blank `title` | accepted; the id becomes the label |
| `refresh_ms` of 1–999 or over 300000 | accepted, clamped to 1000 / 300000 |
| no `panels` key at all | accepted; the plugin contributes no tabs |

Panel ids are hosted as `plugin__<plugin>__<id>`, so they cannot collide with the
built-in inspector tabs (`workspace`, `changes`, `commands`, `agents`). Delimiters
are not an injective encoding across plugins: plugin `a` with panel `b__c` and
plugin `a__b` with panel `c` both produce `plugin__a__b__c`. As with colliding
tools, the first plugin in discovery order keeps the hosted id; every later
claimant is omitted from panel lists and its reads are refused without calling the
plugin. The refusal is logged and reported in that plugin's diagnostics, naming
both the id and the owner. This prevents tabs, caches and payloads from aliasing.
The icon is a *name* from the app's own icon set
(`check`, `circle-check`, `clipboard`, `copy`, `layers`, `list`, `eye`, `search`,
`spark`, `folder`, `folder-open`, `file`, `file-code`, `git`, `branch`,
`terminal`, `agents`, `bolt`, `cpu`, `chat`, `lock`, `alert`, `upload`,
`download`, `thumb`); an unknown name draws a neutral glyph rather than failing.

### UI contributions: buttons, commands and hides

A panel is a tab. The `ui` array in the manifest is the rest of the chrome: a
button in a named slot, an entry in the command palette, and a request to hide
one of the app's own controls. Everything is **declarative** — a name from a set
the app owns — so a plugin still ships no markup, no CSS and no script, and the
app is the only thing that draws.

```json
{"name":"kc-focus","version":"1.0.0","protocol":"1",
 "ui":[
   {"kind":"button","id":"focus","label":"Focus mode","icon":"bolt",
    "slot":"composer.toolbar","action":"focus"},
   {"kind":"command","id":"plan","label":"Ask about the plan",
    "message":"Summarise the plan for this workspace."},
   {"kind":"keybinding","id":"focus-key","label":"Focus mode shortcut",
    "keys":"mod+shift+f","action":"focus"},
   {"kind":"hide","target":"inspector.tab.agents"}
 ]}
```

In Rust the SDK supplies `.button(slot, id, label, handler)`,
`.button_with(…)`, `.message_button(…)`, `.command(…)`, `.command_with(…)`,
`.message_command(…)`, `.keybinding(id, label, keys, handler)`,
`.message_keybinding(…)`, `.hide(target)` and `.on_ui_action(action, handler)`; the
raw `.ui(PluginUiContribution { … })` is there for anything they do not cover.

**Two ways to activate a control.** `action` names a `ui/action` handler inside
the plugin (§4.9) and can answer with a toast, panels to refresh and a URL to
open. `message` posts that text as a user turn — the agent does the work, so no
handler is needed. Exactly one of the two; declaring both or neither fails the
handshake. Two controls may share one action: a button and a chord over the same
code is the normal case, not a special one.

**Slots** — the closed list of places a button may land:

| Slot | Where it is drawn |
| --- | --- |
| `composer.toolbar` | the composer's action row, beside the model controls |
| `inspector.header` | the results panel's own header |
| `session.header` | the session header, before the built-in chips |

**Keybindings** — a `keybinding` is not drawn anywhere; the app matches it:

* `keys` is `+`-joined: one or more of `mod` (Ctrl on Windows/Linux, Cmd on macOS),
  `ctrl`, `alt`, `shift`, then a single key — a letter, a digit, or one of `enter`,
  `escape`, `space`, `/`, `[`, `]`, `,`, `.`, `-`. `Shift+Mod+F` and `mod+shift+f`
  are the same binding; the host canonicalises it.
* **A modifier is required.** A bare key is typing, and `shift` alone is still
  typing, so neither is accepted.
* The app's own shortcuts are refused: `mod+k` (palette), `mod+b` (sessions rail),
  `mod+n` (new session), `mod+j` (context), `mod+shift+t` (theme studio),
  `mod+enter` (send). A contribution naming one is skipped with a note.
* One chord, one handler: if two plugins bind the same chord the first in
  discovery order keeps it and the other is skipped — the keystroke can never run
  two plugins.
* A binding never fires while the caret is in a field (a chord with a modifier is
  still a text-editing key there) or while a dialog is open.
* The palette row of a command whose action is also bound shows the chord, which
  is how a person discovers it.

**Hideable targets** — the closed list of built-in controls a plugin may hide:

| Target | What it hides |
| --- | --- |
| `composer.attach` | the paperclip / attach-files button |
| `inspector.tab.workspace` | the **Files** tab |
| `inspector.tab.changes` | the **Git** tab |
| `inspector.tab.session` | the **Session usage** tab |
| `inspector.tab.agents` | the **Agents** tab |
| `inspector.context` | the **Context** button in the session header |

Hiding is **app-wide** (every session, every project) and it is an allowlist on
both sides: the host refuses anything else, and the browser refuses it again. A
control that stops a run, sends a message or answers an approval is deliberately
absent from the list, so a plugin cannot remove a safety control. The inspector
also refuses to hide its last remaining tab: an unanswered hide request is better
than an empty dock, and the request is reported in the plugin's diagnostics.

Validation, at the handshake (`PluginDiagnostics.error`):

| Declaration | Result |
| --- | --- |
| unknown `kind` | skipped, with a note in the diagnostics |
| unknown `slot` on a button, or no `slot` | skipped / handshake error naming the slot |
| `hide` target not on the list | skipped, with the list in the note |
| `keys` that is not a usable chord, or no `keys` | handshake error naming the accepted shape |
| `keys` the app already uses | skipped, with the app's shortcut list in the note |
| id with anything but `a-z0-9-_.`, or declared twice | handshake error |
| blank label, or a label over 80 bytes | handshake error |
| both `action` and `message`, or neither | handshake error |
| `action` not following the id rule | handshake error |
| more than 64 contributions | handshake error |

The split is deliberate: an item this *build* cannot place (a slot or target from
a newer release) is skipped so the plugin keeps running, while a shape error is
refused loudly so the author fixes it.

A contributed control is always **attributed**: it carries a dot and a tooltip
naming its plugin and version, because a plugin must not be able to pass its
button off as a built-in one. The payload reaches the browser as
`GET /api/config`'s `plugin_ui` (contributions, hidden targets with the plugins
that asked, and the catalogues above), the click goes to
`POST /api/plugins/:name/ui/actions/:action`, and the frontend re-validates
everything before drawing a single control.

### The person's side of it

Adding a control and hiding one are both *requests*. Whoever is looking at the
screen can switch a contributed control off, or put a hidden built-in back,
without disabling the plugin — **Settings → Plugins & MCP → Controls in the
interface** lists every contribution per plugin and every hidden built-in with
the plugin that hid it. The choice is stored in that browser (`localStorage`), so
it needs no approval, no round trip and no config reload; the trade-off is that it
does not follow you to another machine.

The overrides are applied to the same model every component reads, so nothing
downstream knows whether a control is absent because no plugin declared it, because
its plugin was disabled, or because somebody switched it off.

### Rich panels: the `view` vocabulary

A panel payload is data, and the app draws it. Without help that means the generic
JSON tree — correct, and not much of a panel. So the app understands a small set of
**shapes**: return one of these and it is drawn with the product's own components.
Return anything else and the JSON view takes over, which is what keeps an older app
and a newer plugin readable to each other.

```json
{"view": "stat",     "label": "Words in prose", "value": "12,408", "detail": "across 31 files", "tone": "ok"}
{"view": "list",     "title": "Recently touched", "items": [{"text": "docs/PLUGINS.md", "detail": "2 min ago"}]}
{"view": "bars",     "label": "Weight by area", "bars": [{"label": "src", "value": 134413, "display": "131.3 kB"}]}
{"view": "progress", "label": "Snapshots taken", "fraction": 0.3, "detail": "3 of 10"}
{"view": "clock",    "label": "Writing", "remaining_secs": 932, "running": true}
{"view": "stack",    "views": [ …any of the above… ]}
```

* every field is plain text or a number — never markup, a URL, a class name or a
  style;
* `tone` is one of `neutral`, `ok`, `warn`, `bad`, `accent`, and it colours an edge
  or a bar, not the whole card;
* a `bars` entry's `value` decides the width; `display` is what gets printed beside
  it, so a plugin counting bytes can print `131.3 kB`;
* `fraction` is clamped to `0..=1`, `remaining_secs` to a day, text to 120
  characters and a `stack` to three levels — a payload that breaks those rules is
  drawn smaller, never a broken tab;
* give the panel a `refresh_ms` and a **`clock`** really ticks, because each poll
  is a fresh payload. That is the only real-time surface a plugin has, and it is
  enough for a timer, a countdown or a build status.

`view` is optional and additive: a panel that returns `{"items": […]}` still
renders exactly as before.

---

## 9. Writing a plugin

### Rust: `karucode plugin init`

There is no published karucode crate and nothing to point a relative `path` at.
The `karucode` binary writes the SDK **into** the plugin instead, so a Rust
plugin depends on nothing from karucode: it builds in any directory, offline, and
keeps working after the harness is upgraded because the wire protocol is the only
contract between them.

```bash
karucode plugin init ./wordy                 # the name comes from the directory
karucode plugin init ./wordy --name wordy-2  # explicit name (must be a legal crate name)
karucode plugin init ./wordy --force         # replace an existing scaffold
cd wordy && cargo build --release
```

Four files, nothing hidden:

| File | Contents |
| --- | --- |
| `Cargo.toml` | `serde` + `serde_json` only — no karucode dependency of any kind |
| `src/main.rs` | a starter plugin with one `echo` tool — this file is yours |
| `src/protocol.rs` | the wire types, copied from the build that generated it |
| `src/sdk.rs` | the SDK body (`PluginBuilder`, `Plugin::run`, `PluginToolResultExt`) |

The starter's `main` is the whole surface a plugin usually needs:

```rust
use sdk::{PluginBuilder, PluginToolResult, PluginToolResultExt, ToolRequest};
use serde_json::json;

fn main() -> std::io::Result<()> {
    PluginBuilder::new("wordy", "0.1.0")
        .description("Counts words")
        .tool(
            "count",
            "Count the words in `text`.",
            json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
            |req: ToolRequest| {
                let text = req.str_arg("text");
                if text.is_empty() {
                    return PluginToolResult::err("`text` is required");
                }
                PluginToolResult::ok(format!("{} words", text.split_whitespace().count()))
            },
        )
        .tool_with_class("index", "Rebuild the index.", json!({}), Some("mutating"), |_req| {
            PluginToolResult::ok("index rebuilt")   // needs files.write
        })
        .hook(sdk::hooks::BEFORE_PROMPT, |_payload| json!("The wordy plugin is active."))
        .run()
}
```

`PluginBuilder` handles the protocol; `Plugin::handle_request` / `Plugin::run`
are there if you want to embed it differently. `PluginToolResult` constructors
(`ok`, `err`, `content`, `with_meta`) come from `PluginToolResultExt`.

**Keeping the copy current.** The vendored SDK is the one that generated the
project. After upgrading karucode:

```bash
karucode plugin sdk ./wordy           # rewrite src/sdk.rs and src/protocol.rs
karucode plugin sdk ./wordy --check   # exit 1 when stale; use it in the plugin's CI
```

`--check` compares byte-for-byte and never touches `src/main.rs`, so it is safe
in a pre-commit hook. It writes nothing and starts no server.

**Inside the karucode repository**, `plugins/example-hello` is a workspace member
and therefore uses `kc-plugin-sdk.workspace = true`, i.e. a relative path into
`crates/`. That manifest only resolves from inside this checkout: a plugin living
anywhere else must be created with `karucode plugin init` (or copy the example's
*sources*, never its `Cargo.toml`).


### Any language

Requirements: read whole lines from stdin, write whole lines to stdout, keep
stdout clean, exit when stdin closes (or on `shutdown`). ~60 lines of Python:

```python
import json, sys

MANIFEST = {"name": "py-plugin", "version": "0.1.0", "protocol": "1",
            "capabilities": ["tools"], "hooks": []}
TOOLS = [{"name": "upper", "description": "Uppercase text.",
          "parameters": {"type": "object",
                         "properties": {"text": {"type": "string"}}}}]

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    req = json.loads(line)
    method, rid = req.get("method", ""), req.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": TOOLS}})
    elif method == "tools/call":
        name = req["params"]["name"]
        if name == "upper":
            text = req["params"].get("arguments", {}).get("text", "")
            send({"jsonrpc": "2.0", "id": rid,
                  "result": {"ok": True, "content": text.upper()}})
        else:
            send({"jsonrpc": "2.0", "id": rid,
                  "error": {"code": -32601, "message": "unknown tool " + name}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None})
        break
    else:
        send({"jsonrpc": "2.0", "id": rid,
              "error": {"code": -32601, "message": "unknown method " + method}})
```

The test fixtures [`tests/fixtures/fake_plugin.sh`](../tests/fixtures/fake_plugin.sh)
are a working example in POSIX sh + python3, including crash and timeout
behaviour; copy that shape for your own smoke tests.

---

## 10. Testing a plugin

1. Run it by hand and type protocol lines:
   ```bash
   printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol":"1"}}' \
     | ./target/debug/example-hello
   ```
2. Point the host at it (`command = [...]` in `config.toml` or a `plugin.toml`),
   watch `GET /api/plugins`, then call `plugin__<name>__<tool>`.
3. Check stderr output in the server log (`KARUCODE_LOG=kc_plugin_host=debug`).

The host's own suite (`cargo test -p kc-plugin-host`) covers handshake,
`tools/list`, `tools/call`, remote errors, crashes mid-call, timeouts, hook
fan-out, enable/disable, restart capping, permission gating against the shell
fixture, and the config-authoritative permission policy (a manifest cannot grant
itself a permission; ignored manifest permissions appear in diagnostics).