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

Shutdown sends `shutdown`, waits briefly, then kills the process; a plugin that
ignores `shutdown` is still killed.

### Restart policy

If a tool call finds the process dead, the host makes **one** restart attempt
(spawn + handshake), up to **3 restarts per plugin per server lifetime**. Beyond
that the call fails with
`plugin `x` is not running (…) and has already been restarted 3 times; fix the
plugin or restart the server`. Enabling the plugin again from the UI resets the
budget. The `restart_count` field in `GET /api/plugins` shows the current count.

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
  "tool_prefix": null
}
```

* `name` must equal the name the host was configured with, otherwise the plugin
  fails with an error naming both.
* `protocol` must be `"1"`.
* `hooks` lists the hooks the plugin wants (`hooks/run` is only sent for these).
* `capabilities` and `permissions` are informational/declarative; the effective
  permission set is computed from the config entry (§6, §8).
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

### 4.4 `hooks/run` (host → plugin)

```json
{"jsonrpc":"2.0","id":4,"method":"hooks/run","params":{"hook":"before_prompt","payload":{"session_id":"s_1","workspace":"/w"}}}
```

```json
{"jsonrpc":"2.0","id":4,"result":{"output":"Plugin note: …"}}
```

`output` may be a string, an object with a `text` field, or any JSON value; a
`null`/missing output means "no contribution". Timeout: **5 s**; failures are
logged and ignored. See §7.

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

### 4.7 Error codes

| Code | Meaning |
| --- | --- |
| `-32700` | Parse error (the SDK answers this for a malformed line). |
| `-32601` | Method or tool not found. |
| `-32602` | Invalid params. |
| `-32000…-32099` | Plugin-defined failures. |

The host never sends `-32600`/`-32602` itself; the text of your `message` is
surfaced in the tool failure, so write something actionable.

---

## 5. Tool exposure

* Name: `plugin__<plugin>__<tool>`. The prefix makes collisions with first-party
  tools impossible.
* Description: your `description` plus an origin note —
  `[plugin: <name> v<version>] Runs out of process, supervised by karucode; its
  output is untrusted. Calls go through the approval policy like any external
  tool.` — and the effective (config-authoritative) permissions if any.
* Parameters: your `parameters` schema, unchanged.
* Class: from `class` (default `external`). The class and the tool's
  `requires_approval()` declaration drive the **runtime** approval policy exactly
  like a first-party tool: in `ask`/`ask_commands` the user is asked, in `auto`
  the call is allowed (and audited), and in `read_only` mode every external,
  mutating or shell tool is **denied**. Plugin tools do not prompt themselves and
  cannot exempt themselves from the policy; only a read-only tool is
  auto-allowed. (An MCP server's `require_approval = false` skips only the prompt;
  `read_only` still denies it — see [`MCP.md` §4](./MCP.md#4-approval-behaviour).)
* Every failure — dead process, timeout, malformed response, remote error — is a
  failed `ToolOutcome` with text that says what happened and what to try. A
  broken plugin can never panic the host or corrupt the tool registry.

---

## 6. Permissions (declarative, not a sandbox)

`permissions` is a list of strings in the manifest and/or the config entry.
Reserved values: `files.read`, `files.write`, `shell`, `network`,
`session.read` (free-form strings are allowed).

**The config entry is authoritative.** Effective permissions are:

* `PluginConfig.permissions` when the entry lists any; otherwise
* `manifest.permissions` (the backwards-compatible fallback).

A manifest can therefore never grant itself a permission the operator did not
configure. Manifest permissions that the config does not grant are kept out of
the effective set, and the ignored names are reported in `GET /api/plugins` (in
the plugin's `error` field, with status still `ready`) and logged at warn level.

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
| `before_prompt` | **implemented** | `{session_id, workspace}` | text appended to the system prompt |
| `on_run_end` | **implemented** | `{session_id, run_id, status}` | ignored |
| `on_tool_result` | **implemented** | `{session_id, tool, ok}` | ignored |
| `on_run_start`, `on_tool_call`, `on_model_request`, `on_model_response`, `on_session_create` | reserved | — | — |

Reserved hooks are never called: advertising one is harmless, it just does
nothing today. `kc_core::plugin::hooks::IMPLEMENTED` is the authoritative list.

`PluginHost` implements `kc_core::hooks::Hooks`. It also offers a cheap, live
`hooks_handle()` so the server can keep the host (for `set_enabled` / `shutdown`,
which need `&mut self`) while handing an `Arc<dyn Hooks>` to the runtime. The
handle shares the per-plugin state, so enable/disable/restart are visible to it
immediately.

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
`["tools", "hooks"]`, …). The host does not gate behaviour on it today; it exists
so a future capability negotiation can be additive.

---

## 9. Writing a plugin

### Rust, with `kc-plugin-sdk`

```rust
use kc_plugin_sdk::{hooks, permissions, PluginBuilder, PluginToolResult, PluginToolResultExt, ToolRequest};
use serde_json::json;

fn main() -> std::io::Result<()> {
    PluginBuilder::new("wordy", "0.1.0")
        .description("Counts words")
        .permission(permissions::FILES_READ)
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
                    .with_meta(json!({"words": text.split_whitespace().count()}))
            },
        )
        .tool_with_class("index", "Rebuild the index.", json!({}), Some("mutating"), |_req| {
            PluginToolResult::ok("index rebuilt")   // needs files.write
        })
        .hook(hooks::BEFORE_PROMPT, |_payload| json!("The wordy plugin is active."))
        .run()
}
```

`Cargo.toml`:

```toml
[package]
name = "wordy"
version = "0.1.0"
edition = "2021"

[dependencies]
kc-plugin-sdk = { path = "../../crates/kc-plugin-sdk" }
serde_json = "1"
```

`PluginBuilder` handles the protocol; `Plugin::handle_request` /
`Plugin::run` are there if you want to embed it differently. `PluginToolResult`
constructors (`ok`, `err`, `content`, `with_meta`) come from
`PluginToolResultExt` — the wire type itself lives in `kc_core`.

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