# karucode HTTP API (v1)

All responses are JSON unless noted. Errors use:

```json
{ "error": { "code": "bad_request", "message": "human readable" } }
```

Codes: `bad_request`, `not_found`, `permission_denied`, `unauthorized`, `config`,
`provider`, `tool`, `store`, `plugin`, `timeout`, `path_escape`,
`payload_too_large`, `internal`.

## Auth and CSRF

* Default bind is `127.0.0.1`, no auth.
* If `KARUCODE_AUTH_TOKEN` is set, every request needs `Authorization: Bearer <token>`
  (the SSE stream uses `fetch`, so headers work). `GET /api/events` additionally
  accepts `?token=<token>`, because some stream clients cannot set headers; the
  token then appears in the URL, so prefer the header when you can.
* Every state-changing request (POST/PATCH/PUT/DELETE) must send
  `X-Karucode-Client: web`. Requests with an `Origin` header must have an origin
  whose authority matches the `Host` header (case-insensitive; values containing
  whitespace or control characters are rejected). This blocks simple cross-site
  form posts and DNS-rebinding-style cross-origin writes.
* While bound to loopback **without** a token, the request `Host` must be
  `127.0.0.1`, `localhost` or `[::1]` (any port), plus anything listed in
  `KARUCODE_ALLOWED_HOSTS` (`host`, `host:port`, or `*.example.com`). This is what
  defeats DNS rebinding; anything else is `403 permission_denied`.

## Health / config

`GET /api/health`
```json
{ "status": "ok", "version": "0.1.0", "uptime_secs": 12,
  "workspace": "/home/me/project",
  "projects": [ { "id": "p_ab12", "name": "project", "path": "/home/me/project",
                  "created_at": 1710000000000, "updated_at": 1710000000000,
                  "session_count": 2 } ],
  "auth_required": false, "provider_configured": true, "db": "ok" }
```
`status` is `"ok"` when the database probe passes and `"degraded"` otherwise
(the endpoint still returns 200 so a supervisor can see the reason). `workspace`
is the **default** workspace (the `--workspace` / `KARUCODE_WORKSPACE`
directory), not a project path; `projects` is the same list `GET /api/projects`
returns, so one request is enough to map a session's `project_id` to a
directory. `provider_configured` is computed from the effective model (below).

`GET /api/config`
```json
{
  "workspace": "/home/me/project",
  "model": { "provider": "openai_compatible", "base_url": "http://127.0.0.1:4000/v1",
             "model": "gpt-4o-mini", "temperature": 0.2, "max_tokens": null,
             "top_p": null, "request_timeout_secs": 300,
             "parallel_tool_calls": true, "api_key_set": false },
  "providers": [
    { "id": "gateway", "name": "Gateway", "provider": "openai_compatible",
      "base_url": "https://gw.example/v1",
      "models": [ { "id": "coder", "name": "Coder", "context_window": 128000,
                    "max_output_tokens": 16384, "tool_call": true,
                    "reasoning": false, "cost_input": 0.25,
                    "cost_output": 1.5, "catalog_provider": "openai" } ],
      "request_timeout_secs": 300, "api_key_set": true } ],
  "active": { "provider": "gateway", "model": "coder" },
  "limits": { "max_iterations": 200, "max_run_secs": 86400,
              "auto_continue": true, "max_continues": 50,
              "max_tool_output_bytes": 24000, "max_command_output_bytes": 32000,
              "command_timeout_secs": 120, "max_command_timeout_secs": 600,
              "max_file_read_bytes": 200000, "search_max_results": 200,
              "context_max_chars": 240000, "max_tool_calls_per_turn": 32 },
  "approvals": { "mode": "ask", "allow_commands": [], "auto_approve_paths": [],
                 "timeout_secs": 300 },
  "subagents": { "max_concurrent": 3, "max_iterations": 200,
                 "max_continues": 60, "allow_recursive": false,
                 "max_depth": 1, "timeout_secs": 14400 },
  "plugins": [ { "name": "example-hello", "version": "0.1.0", "status": "ready",
                 "tools": ["plugin__example-hello__hello"], "error": null,
                 "permissions": ["files.read"] } ],
  "mcp_servers": [ { "name": "fs", "transport": "stdio", "status": "connected",
                     "tools": ["mcp__fs__read"], "error": null } ],
  "settings_locked": false,
  "available_providers": ["openai_compatible"],
  "version": "0.1.0"
}
```

`model` is the **effective** connection: the active provider profile resolved into
one `ModelConfig` (`base_url`, provider id, model id, timeout and the profile's
key). With no profiles configured a synthetic `default` profile holds the flat
`[model]` section, so the field is always populated. `temperature`, `max_tokens`,
`top_p` and `parallel_tool_calls` are install-wide and come from the flat section
regardless of which profile is active.

`providers[]` entries are `ProviderProfile` values; the key itself is never
serialised, only `api_key_set`. `extra_headers` / `extra_body` are config-file only
and are neither returned here nor accepted by `PATCH`. Optional `ModelEntry`
fields are omitted when unset. `active` is the selection the runtime resolves on
every run: `provider` matches a profile id, `model` is the id sent with the
request (normally an entry of that profile, or the flat `model.model` while a
profile has no models yet).

`PATCH /api/config` — any subset; returns the same shape as `GET /api/config`.

```json
{ "model": { "base_url": "http://gw:4000/v1", "model": "coder", "api_key": "sk-..." },
  "providers": { "upsert": [ { "id": "local", "base_url": "http://127.0.0.1:8000/v1",
                               "models": [ { "id": "qwen3-32b" } ] } ],
                 "delete": [ "old" ] },
  "active": { "provider": "local", "model": "qwen3-32b" },
  "approvals": { "mode": "ask_commands" },
  "limits": { "max_iterations": 40 } }
```

Every section is optional and unknown fields are rejected (`400 bad_request`), so a
misspelt key cannot silently do nothing.

Write-only keys:

* `model.api_key` and `providers.upsert[].api_key` are stored, kept in memory and
  never returned; `api_key_set` is the only signal. An empty string counts as
  absent.
* `clear_api_key: true` erases the stored key and wins over an `api_key` in the
  same request. The cleared state is persisted, so it also shadows
  `KARUCODE_API_KEY` / `OPENAI_API_KEY` from then on.

Provider profiles:

* `providers.upsert[].id` selects an existing profile or creates one. Fields left
  out keep their current value; `models`, when present, **replaces the whole
  list**. A new profile defaults to `name` = id, `provider` =
  `openai_compatible`, `base_url` = `http://127.0.0.1:4000/v1`,
  `request_timeout_secs` = 300, no models and no key.
* `providers.delete[]` removes profiles by id. Deleting the active profile clears
  the selection and the first remaining profile becomes active. Deleting the last
  profile does not leave zero profiles: a `default` profile is re-synthesised from
  the flat `model` section, which still holds the mirrored connection.
* `active.provider` / `active.model` select the runtime's model. An unknown
  provider, or a model that is not in that profile's list, is `400 bad_request`.
  `active.model` may only be omitted when the selected profile has no models, so
  removing a model from a list requires also selecting a different one in the
  same request. An empty `active.provider` is ignored; an empty `active.model`
  clears the selection and is then validated as above.
* At most 16 profiles and 64 models per profile, with the limits below.

Legacy flat patches: `model.provider`, `model.base_url`,
`model.request_timeout_secs`, `model.api_key` and `model.clear_api_key` are
applied to the active profile, so a script written against the flat fields keeps
working and the two views cannot drift. `model.model` is appended to the active
profile's model list if it is not already there, and becomes `active.model`.
`model.temperature`, `model.max_tokens`, `model.top_p` and
`model.parallel_tool_calls` stay install-wide.

Validation (all `400 bad_request`, message names the field and the limit):

| Field | Rule |
| --- | --- |
| profile `id` | 1–64 chars, letters/digits/`.`/`_`/`-`, must start with a letter or digit; duplicates (possible only in a config file) are rejected |
| profile `name` | ≤80 characters, no control characters |
| profile `provider` | one of `available_providers` (today only `openai_compatible`) |
| profile `base_url` | `http://` or `https://`, non-empty host, optional numeric port, no embedded credentials (`user@`), no whitespace or control characters, IPv6 hosts bracketed |
| profile `request_timeout_secs` | 1–3600 |
| profile `extra_headers` (file only) | non-empty header names, no CR/LF in names or values |
| model `id` | non-empty, ≤200 characters, no control characters, unique within the profile |
| model `name` | ≤200 characters, no control characters |
| `context_window`, `max_output_tokens` | 1–100000000 tokens (0 is rejected, so a unit mistake is caught) |
| `cost_input`, `cost_output` | 0–1000000 USD per million tokens, finite |

Values are range-checked; out-of-range numbers are rejected rather than silently
clamped. Response `403 permission_denied` when `settings_locked` is true, checked
before the body is parsed.

`POST /api/provider/test` — probes an endpoint (`GET /models`, falling back to a
1-token chat). Every field of the body is optional and the body itself may be
empty:

```json
{ "profile": "gateway", "base_url": "https://draft.example/v1",
  "api_key": "sk-...", "model": "coder", "request_timeout_secs": 60 }
```

* no body → the active model, exactly as the runtime would use it;
* `profile` → that saved profile instead (its key is used unless `api_key` is
  given); an unknown id is `404 not_found`;
* `base_url`, `model`, `api_key` and `request_timeout_secs` override the chosen
  configuration for this probe only (`request_timeout_secs` is clamped to
  1–3600; an empty `api_key` drops the key for the probe). Nothing is saved.

Returns `200` with `{ "ok": bool, "models": [..], "error": null,
"latency_ms": 123 }`. A failing endpoint is a *result*, not an HTTP error: a bad
`base_url`, an empty model id, a provider that cannot be built and an unreachable
host all come back as `ok: false` with `error` set (secrets redacted).
`models` is empty when the endpoint has no `/models` listing but the chat probe
succeeded.

## Model catalog

`GET /api/models/catalog[?provider=<id>][&refresh=true]` serves models.dev
metadata for prefilling the settings screen. The server fetches it, not the
browser: the UI's CSP is `connect-src 'self'` and the document is several MiB.

Without `provider`, the provider list:

```json
{ "ok": true,
  "providers": [ { "id": "openai", "name": "OpenAI", "model_count": 42 } ],
  "models": [], "fetched_at_ms": 1710000000000, "stale": false, "error": null }
```

With `provider=<id>`, that provider's models with the fields the settings screen
prefills:

```json
{ "ok": true, "providers": [],
  "provider": { "id": "openai", "name": "OpenAI", "model_count": 42 },
  "models": [ { "id": "gpt-4o-mini", "name": "GPT-4o mini",
                "context_window": 128000, "max_output_tokens": 16384,
                "tool_call": true, "reasoning": false,
                "cost_input": 0.15, "cost_output": 0.6 } ],
  "fetched_at_ms": 1710000000000, "stale": false, "error": null }
```

`provider` here is a **models.dev** provider id, not a karucode profile id.
Optional model fields are omitted when models.dev does not report them; providers
with no models are left out, and models are sorted by display name then id.

* The parsed catalog is cached in memory for 6 hours. `fetched_at_ms` is when the
  served copy was fetched; `refresh=true` (`1|true|yes` / `0|false|no`) forces a
  fetch. A burst of requests shares one upstream fetch.
* Source: `https://models.dev/api.json`, or `KARUCODE_MODELS_DEV_URL` (read once
  at server start; a value that is not `http://` or `https://` makes every fetch
  fail with `ok: false`) for a mirror or an air-gapped install. Fetch timeout
  20s; a body larger than 32 MiB is refused.
* A failed refresh with a cached copy still answers `200 {ok: true}` with that
  copy: `stale` is `true` and `error` carries the reason.
* With nothing cached, the answer is `200 {ok: false, providers: [], models: [],
  fetched_at_ms: 0, error: "..."}`. A missing catalog degrades the prefill; it
  never fails the request.
* An unknown provider id is `404 not_found` when a catalog is available. When the
  fetch itself failed there is no catalog to look the id up in, so the `ok:false`
  body above is returned instead.

### Model metadata lookup

`GET /api/models/lookup?model=<id>` finds the best catalog entry for one
arbitrary model id — the metadata detector behind "add a model id by hand".
Gateways fronting several providers expose ids with routing prefixes
(`a6api/deepseek-v4.1-flash-1017`, or `a6api/ali/deepseek-v4.1-flash-1017` with
an upstream segment), so the whole id is tried first (catalog ids can
themselves contain slashes, like OpenRouter's `openai/gpt-4o`), then leading
`segment/` prefixes are stripped one at a time until a suffix matches. Matching
is case-insensitive; when several providers carry the winning suffix, a
provider whose id appears as a segment of the queried id wins, then ties break
alphabetically.

```json
{ "ok": true,
  "match": { "provider": "deepseek",
             "matched_id": "deepseek-v4.1-flash-1017",
             "model": { "id": "deepseek-v4.1-flash-1017",
                        "name": "DeepSeek V4.1 Flash",
                        "context_window": 128000 } },
  "fetched_at_ms": 1710000000000, "stale": false, "error": null }
```

* A missing `model` parameter is `400 bad_request`.
* A model the catalog does not carry is an ordinary miss: `200 {ok: true,
  match: null}` — the caller adds the id without metadata.
* Same availability contract as the catalog endpoint: an unreachable catalog
  with nothing cached answers `200 {ok: false, match: null, error: "..."}`.

`GET /api/stats`
```json
{ "uptime_secs": 42, "rss_kb": 38211, "sessions": 3, "active_runs": 1,
  "events_emitted": 512, "db_path": "/home/me/.local/share/karucode/karucode.db" }
```
`rss_kb` is read from `/proc/self/status` and is `0` on platforms without it.
`events_emitted` is the number of events published on the shared sequence.

## Projects

A project is a directory the agent may work in plus a display name. Sessions can
belong to one, and a run is confined to its session's project directory. The
default workspace (`--workspace` / `KARUCODE_WORKSPACE`) is the directory for
sessions that belong to no project, and it is what a request without `?project=`
uses.

`GET /api/projects` →
```json
{ "projects": [ { "id": "p_ab12", "name": "karucode", "path": "/home/me/karucode",
                  "created_at": 1710000000000, "updated_at": 1710000000000,
                  "session_count": 3 } ],
  "workspace": "/home/me/project" }
```
`workspace` is the default workspace root, not a project path. `path` is the
canonical directory and is unique across projects.

`POST /api/projects` body `{ "name": "optional", "path": "/abs/dir" }` →
`{ "project": Project }` (`200`)

* `path` is required, must be absolute and an existing directory, and is
  canonicalised before use — two spellings of the same directory, or a symlink and
  its target, are one project, and adding it twice is `400 bad_request` naming the
  existing project.
* `400 bad_request` for a missing body, a relative or non-existent path, a path
  that is a file, or a 65th project (at most 64 are supported).
* `name` defaults to the directory's basename (`"Project"` when it has none). An
  empty, over-long (>80 characters) or control-character name is **not** rejected
  here: the basename is used instead. `PATCH` is the strict path.

`PATCH /api/projects/:id` body `{ "name": "..." }` → `{ "project": Project }`  
`404 not_found` for an unknown id; `400 bad_request` when the name is empty,
longer than 80 characters or contains control characters.

`DELETE /api/projects/:id[?force=true]` →
`{ "deleted": true, "deleted_sessions": 2 }`

* `400 bad_request` while the project still has sessions, unless `force=true`; the
  message names the session count.
* With `force=true` those sessions are deleted (messages, runs, agents, approvals
  and claims cascade with them) and `deleted_sessions` reports how many.
* `400 bad_request` when one of those sessions has a run in flight; a running
  session is never killed by a project delete. Stop it first.
* Files on disk are never touched — only rows are removed.

### Scoping a request to a project

`GET /api/workspace/tree`, `GET /api/workspace/file`, `POST /api/workspace/upload`,
`GET /api/git/status` and `GET /api/git/diff` accept `?project=<id>` and resolve
paths inside that project's canonical directory. An unknown id is `404
not_found`; so is a project whose directory has since been removed (the message
says the directory may be gone). Omitting the parameter uses the default
workspace; for the upload it falls back to the session's own project.

## Sessions

`GET /api/sessions[?project=<id>]` → `{ "sessions": [SessionSummary] }`
```json
{ "id": "s_ab12", "title": "Fix login bug", "workspace": "/w",
  "project_id": "p_ab12",
  "model": { "provider": "local", "model": "qwen3-coder" },
  "created_at": 1710000000000, "updated_at": 1710000001000,
  "message_count": 12, "last_run_status": "completed", "running": false }
```
`project=<id>` filters to one project; an unknown id is `404 not_found` (not an
empty list). `project_id` is `null` for a session that is not attached to a
project; `workspace` is the directory the session records. `model` is the
per-session model pin (see below) and is **absent** when the session follows the
install-wide default.

`POST /api/sessions` body `{ "title": "optional", "project": "p_ab12",
"model": { … } }` → `Session`
```json
{ "id": "s_ab12", "title": "New session", "workspace": "/w",
  "project_id": "p_ab12", "created_at": 1710000000000,
  "updated_at": 1710000000000 }
```
The body may be empty, which is what a client that predates projects sends:
title defaults to `"New session"` and the session is created in the default
workspace with no project. An unknown `project` is `404 not_found`; `workspace` is
that project's directory. The optional `model` pins the new session immediately
(it is validated before anything is written; a bad pin is `400 bad_request`).

`GET /api/sessions/:id`
```json
{ "session": Session, "messages": [StoredMessage], "runs": [RunRecord],
  "agents": [AgentRecord], "approvals": [ApprovalRecord],
  "claims": [ClaimRecord], "running": false }
```
`approvals` contains **pending** approvals only; resolved and expired decisions are
not exposed by any endpoint yet. `messages` is the most recent page (newest 400,
returned in chronological order).

When the model asks for more tool calls than `limits.max_tool_calls_per_turn` in
one turn, the extra calls are not executed: each one gets a `tool` message
`[skipped: per-turn tool call limit reached …]` and a `notice` event is emitted.

`PATCH /api/sessions/:id` `{ "title": "..." }` → `Session`

### Per-session model selection

A session can be **pinned** to a provider profile + model, so sessions running at
the same time use different models. The install-wide default (`active.provider` /
`active.model`) still applies to every unpinned session and to new sessions;
pinning one session never changes another.

```json
"model": { "provider": "local", "model": "qwen3-coder", "reasoning_effort": "high" }
```

* `reasoning_effort` is optional: `none`, `minimal`, `low`, `medium`, `high` or
  `max`. It is sent as the `reasoning_effort` body field of every model request
  for that session, and the endpoint decides what it supports.
* The pin is validated against the live provider profiles: an unknown `provider`,
  a model the profile does not list, or an unknown effort is `400 bad_request`
  naming the field.
* If the pinned profile or model is deleted later, the run falls back to the
  install-wide default (or that profile's first model) instead of failing.
* Subagents spawned from a session inherit its pin (the `spawn_agent` tool's own
  `model` argument still overrides the model id for that one child).

Two equivalent ways to set or clear it, both returning the updated `Session` and
emitting `session_model_changed`:

`PATCH /api/sessions/:id/model` — the body **is** the selection, `{}` clears it:
```json
{ "provider": "local", "model": "qwen3-coder", "reasoning_effort": "high" }
```

`PATCH /api/sessions/:id` — `{ "model": { … } }` sets, `{ "model": null }`
clears; `title` and `model` may be sent in one request. A request with neither is
`400 bad_request`.  
`DELETE /api/sessions/:id` → `{ "deleted": true }`

`POST /api/sessions/:id/messages` `{ "content": "..." }`
→ `202 { "run_id": "r_...", "session_id": "s_..." }`
Starts a run in the background; progress arrives on the event stream.
Returns `409` with `code: "bad_request"` if a run is already active for that
session (the HTTP status carries the conflict, the code stays in the documented
set).

`POST /api/sessions/:id/cancel` → `{ "cancelled": true }` — `false` when no run is
active (already finished or never started).

### Shapes

`StoredMessage`
```json
{ "id": 42, "session_id": "s_ab12", "run_id": "r_x", "agent_id": null,
  "role": "assistant", "content": "...", "tool_calls": [ { "id": "call_1",
  "name": "read_file", "arguments": "{\"path\":\"a\"}" } ],
  "tool_call_id": null, "name": null, "created_at": 1710000000000 }
```
`role` ∈ `system|user|assistant|tool`.

**Optional fields are omitted, not sent as `null`.** Any field shown below as
nullable (`error`, `finished_at`, `tool_call_id`, `name`, `agent_id`, …) is left
out of the JSON entirely when it is empty, to keep payloads small. Treat a
missing key the same as `null`.

`RunRecord`
```json
{ "id": "r_x", "session_id": "s_ab12", "status": "running", "model": "gpt-4o-mini",
  "prompt": "...", "error": null, "started_at": 0, "finished_at": null }
```
`status` ∈ `running|completed|failed|cancelled|interrupted|limit_reached`.

`AgentRecord`
```json
{ "id": "a_x", "session_id": "s_ab12", "parent_run_id": "r_x", "parent_agent_id": null,
  "name": "reviewer", "role": "reviewer", "task": "Review the diff", "status": "running",
  "model": "gpt-4o-mini", "iterations": 3, "summary": "", "error": null,
  "owned_paths": ["crates/foo/**"], "created_at": 0, "updated_at": 0, "finished_at": null }
```
`status` ∈ `running|completed|failed|cancelled|interrupted`.

`ApprovalRecord`
```json
{ "id": "ap_x", "session_id": "s_ab12", "run_id": "r_x", "agent_id": null,
  "kind": "shell_command", "title": "Run cargo test", "detail": "cargo test",
  "payload": { "command": "cargo test" }, "status": "pending", "decision": null,
  "created_at": 0, "resolved_at": null }
```
`kind` ∈ `shell_command|file_write|file_edit|delete_file|network_tool|plugin|mcp|other`.
`status` ∈ `pending|resolved|expired`.

`ClaimRecord`
```json
{ "path": "crates/foo/src/lib.rs", "session_id": "s_ab12", "agent_id": "a_x",
  "mode": "write", "created_at": 0 }
```

## Approvals

`GET /api/approvals?session=s_ab12` → `{ "approvals": [ApprovalRecord] }` (pending only)  
`POST /api/approvals/:id` `{ "decision": "allow_once" | "allow_session" | "deny" }`
→ `{ "resolved": true, "decision": "allow_once" }`. `404` when unknown/expired.

## Workspace files

`GET /api/workspace/tree?path=.&depth=2[&project=<id>]` →
```json
{ "path": ".", "entries": [
  { "name": "src", "path": "src", "kind": "dir", "size": 0 },
  { "name": "Cargo.toml", "path": "Cargo.toml", "kind": "file", "size": 812 } ] }
```
`depth` is clamped to 1–6 and the listing stops at 2000 entries. Directories that
are never useful in a UI (`node_modules`, `.git`, `target`, `dist`, `.next`,
`.turbo`, `.venv`, `venv`, `__pycache__`, `.pytest_cache`, `.mypy_cache`,
`coverage`, `.idea`) are skipped before the entry budget is spent, so a listing
of a real project is not consumed by its build output or Git internals. They are
still reachable by naming one explicitly (`?path=.git`) and by the file tools.
`project` selects the directory (see [Scoping a request to a project](#scoping-a-request-to-a-project)).

`GET /api/workspace/file?path=src/main.rs[&project=<id>]` →
```json
{ "path": "src/main.rs", "content": "...", "size": 812, "truncated": false,
  "binary": false, "language": "rust" }
```
`404` for missing files, `400 path_escape` outside the workspace. `project`
selects the directory, so a file opened from a project's tree is read from that
project (see [Scoping a request to a project](#scoping-a-request-to-a-project)).

### Attaching a file

`POST /api/workspace/upload?session=<id>&name=<file name>[&project=<id>]` takes
the **raw file bytes** as the body — not JSON, not `multipart/form-data` — and
answers `201`:

```bash
curl -s -X POST "localhost:8787/api/workspace/upload?session=s_ab12&name=report.pdf" \
  -H 'X-Karucode-Client: web' -H 'Content-Type: application/pdf' \
  --data-binary @report.pdf
```
```json
{ "session": "s_ab12", "name": "report-1.pdf",
  "path": ".karucode/attachments/s_ab12/report-1.pdf", "size": 214233,
  "content_type": "application/pdf", "workspace": "/home/me/project",
  "deduplicated": true }
```

This is how the composer hands the agent a file it can actually read: the bytes
are written under the session's project in `.karucode/attachments/<session>/`, and
`path` is workspace-relative, so it can be passed straight back to
`GET /api/workspace/file?path=...` or `@`-mentioned in a prompt. The echoed
`content_type` comes from the request header with its parameters dropped, and is
`application/octet-stream` when the request had none.

Rules:

* `session` must be a real session and match `[A-Za-z0-9_-]{1,64}`; it also picks
the project when `project` is omitted. A malformed id is `400 bad_request`, an
unknown one `404 not_found`.
* `name` is one path segment: no `/`, `\`, `..`, control characters, empty
string, or more than 160 bytes. Spaces and unicode are fine. Anything else is
`400 bad_request` and nothing is written.
* An existing name is never overwritten: the file becomes `report-1.pdf`,
`report-2.pdf`, … and `deduplicated` is `true`. Two concurrent uploads of the
same name both survive (the create is `O_EXCL`). The endpoint is not idempotent:
retrying after a timeout stores a second copy rather than replacing the first.
* The body must be non-empty and at most **8 MiB**; larger is
`413 payload_too_large` (checked against `Content-Length` when present, and
while reading otherwise), empty is `400 bad_request`.
* A `multipart/form-data` or `application/x-www-form-urlencoded` body is
`400 bad_request` — it would be stored verbatim — and a malformed `Content-Type`
is rejected the same way.
* If `.karucode` or `.karucode/attachments` is a symlink that resolves outside
the project, the upload is `400 path_escape`. On Unix the session directory is
created `0700` and the file `0600`.

## Git

`GET /api/git/status[?project=<id>]` →
```json
{ "is_repo": true, "branch": "main", "error": null,
  "files": [ { "path": "src/x.rs", "status": "M", "staged": false } ] }
```
`GET /api/git/diff?path=&staged=false&base=[&project=<id>]` →
```json
{ "diff": "diff --git ...", "truncated": false }
```
Both take the project-scoped query parameter described above; `git` is run with
`current_dir` set to that project's directory. `status` reports failures in the
payload (`200` with `is_repo: false` and `error` set) including a missing `git`
binary; `diff` answers `400 bad_request` when the directory is not a git
repository and `500 internal` when `git` cannot be spawned.

## Plugins and MCP

`PATCH /api/config` and `GET /api/config` both return `plugins`/`mcp_servers` from
the **live** host and manager, not from startup config, so status changes are
visible immediately.

`GET /api/plugins` → `{ "plugins": [PluginDiagnostics] }`
```json
{ "name": "example-hello", "status": "ready|disabled|starting|failed|stopped",
  "manifest": { "name": "...", "version": "0.1.0", "protocol": "1",
                "capabilities": ["tools"], "permissions": ["files.read"],
                "hooks": ["before_prompt"] },
  "tools": [ { "name": "hello", "description": "...", "parameters": {} } ],
  "error": null, "pid": 1234, "restart_count": 0 }
```

`POST /api/plugins/:name/enable` `{ "enabled": true }` → `PluginDiagnostics`

`GET /api/mcp` → `{ "servers": [ { "name": "fs", "transport": "stdio",
  "status": "connected|failed|disconnected", "tools": ["mcp__fs__read"],
  "error": null } ] }`

## Event stream

`GET /api/events?session=<optional session id>` — `text/event-stream`.
Use `fetch()` plus a `ReadableStream` reader (not `EventSource`, so the auth
header travels). Each event is:

```
data: {"seq":12,"ts_ms":1710000000123,"session_id":"s_x","run_id":"r_y","agent_id":null,"type":"text_delta","delta":"Hello"}\n\n
```

A `: keepalive` comment is sent every 15 seconds. The envelope always has
`seq`, `ts_ms`, and optional `session_id`, `run_id`, `agent_id`; the event fields
are flattened alongside `type`.

There is no replay cursor, so the client has to notice what it missed:

* `seq` is monotonic within one server process (it starts again at 1 with the
  process, which is why dedupe is scoped to a connection). A jump means frames
  were dropped - refetch the session with `GET /api/sessions/:id` (messages, runs
  and agents are persisted). On a stream opened with `?session=`, holes are
  normal: other sessions' frames are stamped and then filtered out, so only treat
  a hole as loss on an unfiltered stream.
* The server also says so explicitly: a `stream_lagged` event carries `skipped`,
  the number of frames this client lost. Its `seq` is the last *dropped* one, so
  it is newer than anything the client has seen and older than the next live
  frame - it never shadows a real event. Treat the marker as "refetch now"; it
  describes the transport, not the session, and is exempt from `seq` dedupe.
* Keepalives count as traffic. A connection that delivers no byte for ~45 s is
  dead even though the socket never errored (sleep, Wi-Fi change, proxy or server
  restart without a FIN); reconnect instead of waiting.
* A client that cannot drain its buffer for 5 s is disconnected rather than
  slowing the agent loop. Reconnecting is the documented recovery.
* A refetch must not replace a transcript that is still streaming: the session
  snapshot only contains persisted messages, so replacing the view mid-answer
  deletes what has not been saved yet. Keep the live view and refetch again when
  the run settles.

| `type` | extra fields |
| --- | --- |
| `session_created` | `session_id`, `title` |
| `session_renamed` | `session_id`, `title` |
| `session_model_changed` | `session_id`, `model` (the new pin, or `null` when cleared) |
| `session_deleted` | `session_id` |
| `run_started` | `run_id`, `prompt` |
| `run_finished` | `run_id`, `status`, `error` |
| `run_cancelled` | `run_id` |
| `iteration_started` | `iteration` |
| `stream_retry` | `attempt`, `max_attempts`, `reason` — the provider stream died mid-answer; everything streamed for this turn so far belongs to the abandoned attempt and must be dropped (see `retry_max_stream_restarts`) |
| `text_delta` | `delta` |
| `reasoning_delta` | `delta` |
| `usage` | `input_tokens`, `output_tokens` |
| `tool_call_started` | `call_id`, `name`, `arguments` (object; a **string** containing the raw text when the model emitted invalid JSON) |
| `tool_call_finished` | `call_id`, `name`, `ok`, `output`, `truncated`, `duration_ms` |
| `command_started` | `call_id`, `command`, `cwd` |
| `command_output` | `call_id`, `stream` (`stdout`/`stderr`), `chunk` |
| `command_finished` | `call_id`, `exit_code`, `duration_ms`, `truncated` |
| `approval_requested` | `approval` (ApprovalRecord shape + payload) |
| `approval_resolved` | `approval_id`, `decision` |
| `agent_spawned` | `agent_id`, `name`, `role`, `task`, `model`, `parent_run_id` |
| `agent_progress` | `agent_id`, `iterations`, `note` |
| `agent_finished` | `agent_id`, `status`, `summary`, `error` |
| `notice` | `level`, `message` |
| `stream_lagged` | `skipped` - the SSE pump dropped frames for this client; refetch the session (emitted by the server, not the runtime) |
| `error` | `message`, `kind` |

## Frontend build/dev

Vite dev server proxies `/api` to `KARUCODE_PORT` (default `8787`). In production
the Rust server serves the built `web/dist` (embedded at compile time, or from
`KARUCODE_WEB_DIR` when set).
