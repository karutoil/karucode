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
(the endpoint still returns 200 so a supervisor can see the reason). When the
store fails, `projects` is `[]` and a `db_error` field carries the message, so a
supervisor sees *why* rather than a bare 500. `workspace`
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
             "parallel_tool_calls": true, "supports_images": false,
             "api_key_set": false },
  "providers": [
    { "id": "gateway", "name": "Gateway", "provider": "openai_compatible",
      "base_url": "https://gw.example/v1",
      "models": [ { "id": "coder", "name": "Coder", "context_window": 128000,
                    "max_output_tokens": 16384, "tool_call": true,
                    "reasoning": false, "vision": true, "cost_input": 0.25,
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
                 "max_depth": 1, "timeout_secs": 14400,
                 "allow_background": true, "max_background": 8,
                 "wait_timeout_secs": 600 },
  "notifications": { "enabled": true, "run_finished": true, "needs_input": true,
                     "error": true, "info": true, "plugin": true,
                     "min_run_secs": 0, "say_body": true,
                     "max_min_run_secs": 3600,
                     "kinds": ["run_finished", "needs_input", "error", "info"] },
  "plugins": [ { "name": "example-hello", "version": "0.1.0", "status": "ready",
                 "tools": ["plugin__example-hello__hello"], "error": null,
                 "permissions": ["files.read"] } ],
  "plugin_panels": [ { "plugin": "kc-todo", "version": "0.2.0",
                       "panels": [ { "id": "todo",
                                     "hosted_id": "plugin__kc-todo__todo",
                                     "title": "Todo", "icon": "check",
                                     "source": "static", "refresh_ms": 0 } ] } ],
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

`supports_images` is the install-wide default for image input; `vision` on a
model entry overrides it for that model. The runtime resolves the pair for the
model the session will actually call, so a session pinned to a vision model can
read images even when `supports_images` is `false` and the profile's first model
is text-only. `GET /api/config` reports the effective value for the active
selection; with no profiles it is the flat `[model] supports_images`.

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
misspelt key cannot silently do nothing. A numeric tunable outside its documented
range is the same `400`, naming the field, the range and the value it was given -
nothing is silently clamped.

**`[subagents]` and zero.** Three values in this section are meaningful at `0`, and
they do not agree with each other: `max_concurrent` and `max_background` mean *no
ceiling* (`0..=1024`, with `0` granting every spawn a slot immediately), while
`max_depth` means *no delegation* (`0..=255`, the field being a `u8`) and
`max_continues = 0` restores the one-shot budget. `max_iterations` is `1..=10000`
and `wait_timeout_secs` is `1..=3600`. A `max_depth` above `255` fails JSON
deserialisation rather than being truncated to a smaller number.

**Admin gate.** When `KARUCODE_ADMIN_TOKEN` is set, a patch that changes
`approvals`, `model.base_url`, `model.api_key`, `providers`, `active_provider` or
`active_model` must also send `X-Karucode-Admin-Token` with that value; otherwise
the request is `403 permission_denied` and nothing is applied. Without the gate,
any holder of the ordinary bearer token could turn approvals off and then repoint
the provider at a host of its choosing. Changes to other sections (limits,
subagents, plugins, MCP) need only the normal credential. Every authorised
security change is logged with the field names that changed, never the values.

**Body limits.** JSON routes accept up to 1 MiB + 64 KiB; the upload route accepts
8 MiB + 64 KiB. A body over the limit is `413 payload_too_large` in the same JSON
error shape as every other error, not a plain-text rejection.

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
                "tool_call": true, "reasoning": false, "vision": true,
                "cost_input": 0.15, "cost_output": 0.6 } ],
  "fetched_at_ms": 1710000000000, "stale": false, "error": null }
```

`vision` answers "may `read_file` show this model an image?". It is `true` only
when models.dev lists `image` in `modalities.input`, so an audio-only or
pdf-only model is `false` even though models.dev also marks it `attachment:
true`; the legacy `attachment` flag is used only when the document carries no
`modalities` at all. This is what the settings screen prefills a model's
`vision` field from.

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
                  "session_count": 3, "result_panel_width": 580 } ],
  "workspace": "/home/me/project" }
```
`workspace` is the default workspace root, not a project path. `path` is the
canonical directory and is unique across projects. `result_panel_width` is omitted
until the Workbench result pane has an explicit pixel preference.

`POST /api/projects` body `{ "name": "optional", "path": "/abs/dir" }` →
`{ "project": Project }` (`200`)

* `path` is required, must be absolute and an existing directory, and is
  canonicalised before use — two spellings of the same directory, or a symlink and
  its target, are one project, and adding it twice is `400 bad_request` naming the
  existing project.
* `path` must be an existing directory; `/` itself is refused explicitly.
  Without that rule one request could make the whole filesystem the confinement
  root for every file tool. There is **no allowlist by default** — any other
  directory is a valid project. Set `KARUCODE_PROJECT_ROOTS` (colon-separated) or
  `project_roots` in the config file to restrict creation to those roots,
  compared component-wise on canonical paths; `project_roots` unset means every
  directory is allowed.
* `400 bad_request` for a missing body, a relative or non-existent path, a path
  that is a file, a path outside the allowed roots, or a 65th project (at most 64
  are supported).
* `name` defaults to the directory's basename (`"Project"` when it has none). An
  empty, over-long (>80 characters) or control-character name is **not** rejected
  here: the basename is used instead. `PATCH` is the strict path.

`PATCH /api/projects/:id` body `{ "name": "..." }` → `{ "project": Project }`  
`404 not_found` for an unknown id; `400 bad_request` when the name is empty,
longer than 80 characters or contains control characters.

`PATCH /api/projects/:id/preferences` body `{ "result_panel_width": 580 }` →
`{ "project": Project }` (`200`). This stores only the desktop Workbench result-pane
width for the project, so fresh browser profiles and other devices read it back from
`GET /api/projects`. The integer must be between **280** and **1600** pixels,
inclusive. Missing/fractional/null/string values, unknown fields and out-of-range
widths are `400 bad_request`; an unknown project is `404 not_found`. The current
viewport may show a tighter clamp without persisting that temporary constraint.
Renaming a project leaves this preference unchanged.

`DELETE /api/projects/:id[?force=true]` →
`{ "deleted": true, "deleted_sessions": 2 }`

* `400 bad_request` while the project still has sessions, unless `force=true`; the
  message names the session count.
* With `force=true` those sessions are deleted (messages, runs, agents, approvals
  and claims cascade with them) and `deleted_sessions` reports how many.
* `400 bad_request` when one of those sessions has a run in flight; a running
  session is never killed by a project delete. Stop it first.
* Files on disk are never touched — only rows are removed.

`GET /api/projects/:id/favicon[?max_bytes=N]` → the project's own `favicon.ico`
as `image/*` (`200`), or an empty `204 No Content` when it has none.

* The body is the raw file, not JSON. `Content-Type` is sniffed from the bytes
  (`image/x-icon`, `image/png`, `image/gif`, `image/jpeg`, `image/webp`,
  `image/svg+xml`), so a PNG that is named `favicon.ico` still decodes.
* The search: only the file name `favicon.ico`, matched case-insensitively;
  `.gitignore` rules apply (the project's own, nested ones and
  `.git/info/exclude`, honoured even outside a repository) while the operator's
  global gitignore is not read; hidden directories, `node_modules` and `target`
  are skipped; at most 6 levels and 20000 entries; the shallowest match wins.
* Also `204` for a file over `max_bytes` (default and maximum 512 KiB, a
  non-positive or non-numeric value is `400 bad_request`), an empty file, and a
  symlink pointing outside the project. Two projects never share an icon: the
  search never climbs above the project directory.
* `204` rather than `404` on purpose: browsers log every 4xx as
  `Failed to load resource`, so a project without a favicon — the common case —
  would fill the console with errors. An **unknown project id is still
  `404 not_found`**, because that one is a caller mistake.
* This is what the rail draws on each project square; on `204` the rail uses the
  project's initials. The bytes are fetched with the bearer token and rendered
  from an object URL, so the page CSP's `img-src` must include `blob:`.
  The endpoint is not cached server-side, so a replaced favicon appears on the
  next load.

### Scoping a request to a project

`GET /api/workspace/tree`, `GET /api/workspace/file`, `PUT /api/workspace/file`,
`GET /api/workspace/raw`, `POST /api/workspace/upload`, `GET /api/git/status` and
`GET /api/git/diff` accept `?project=<id>` and resolve paths inside that
project's canonical directory. An unknown id is `404 not_found`; so is a project
whose directory has since been removed (the message says the directory may be
gone). Omitting the parameter uses the default workspace; for the upload it falls
back to the session's own project.

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
  "claims": [ClaimRecord], "running": false,
  "queued_steers": [QueuedMessage], "queued_follow_ups": [QueuedMessage],
  "last_rewind_at": null }
```
`approvals` contains **pending** approvals only; resolved and expired decisions are
not exposed by any endpoint yet. `messages` is the most recent page (default 5000,
maximum 50000, controlled by `?messages=`, returned in chronological order).
Both session and agent detail pages also have an approximately 1 MiB stored-payload
budget; one oversized message is returned alone to guarantee progress.
`messages_truncated` means older visible rows remain and `messages_before` is the
exclusive message ID cursor to pass as `?before=<id>`; otherwise it is null.
The budget counts stored fields, not serialized response bytes. `last_rewind_at` is the unix-ms timestamp of
the session's newest rewind, or `null` if it was never rewound; `messages`
already excludes the hidden rows, so it is informational.

`queued_steers` and `queued_follow_ups` are the messages the user sent while a run
was in flight and that the run has not applied yet (see *Steering a run, following
up, and recalling* below). They are not transcript rows.

When the model asks for more tool calls than `limits.max_tool_calls_per_turn` in
one turn, the extra calls are not executed: each one gets a `tool` message
`[skipped: per-turn tool call limit reached …]` and a `notice` event is emitted.

`PATCH /api/sessions/:id` `{ "title": "..." }` → `Session`

`GET /api/sessions/:id/info` → the numbers behind a session, without a transcript.
This is what the web UI's inspector **Session** tab reads; it stays cheap to poll
on a long run because it carries counts and totals instead of message rows.
```json
{ "session": Session, "running": false,
  "stats": { "messages_total": 481, "messages_main": 402, "messages_subagent": 79,
             "messages_by_role": { "user": 12, "assistant": 61, "tool": 408 },
             "messages_main_by_role": { "user": 9, "assistant": 40, "tool": 353 },
             "tool_calls_total": 408,
             "tool_calls": { "read_file": 210, "run_command": 96 },
             "runs_by_status": { "completed": 11, "failed": 1 }, "runs_total": 12,
             "working_ms": 1842119 },
  "usage": { "main": UsageTotals, "by_agent": { "a_x": UsageTotals },
             "total": UsageTotals },
  "context": { "model": "gpt-4o-mini", "window": 128000, "used": 41023 },
  "agents": [ { "id": "a_x", "name": "reviewer", "role": "reviewer",
                "status": "completed", "model": "gpt-4o-mini", "iterations": 7,
                "duration_ms": 61234, "created_at": 0, "finished_at": 0,
                "messages": 79, "usage": UsageTotals } ],
  "messages": { "total": 481, "main": 402, "subagent": 79,
                "by_role": { … }, "main_by_role": { … } },
  "tools": { "total": 408, "by_name": [ { "name": "read_file", "count": 210 } ],
             "truncated": false },
  "runs": { "total": 12, "by_status": { … }, "working_ms": 1842119,
            "recent": [RunRecord] } }
```
* `stats` counts the **whole** session, not the bounded transcript window, and
  excludes rewound rows. A "turn" has no stored row of its own: it is a `user`
  entry in `messages_main_by_role`, while `runs_total` is how many times the agent
  actually looped over a prompt.
* `tools.by_name` is capped at 40 entries (`truncated: true` when more exist);
  `tools.total` always counts every call.
* `runs.working_ms` sums `finished_at - started_at` over runs that **finished**,
  so it is time the agent worked, excluding idle gaps. An in-flight run is not
  counted until it ends. `recent` carries the newest 20 runs.
* `usage` totals the persisted per-call rows, **per agent**: `main` is the run's
  own model calls, `by_agent` each subagent's, `total` the whole session (exactly
  main plus every agent). This survives a reload — the `usage` event does not.
* `context.used` is the **newest main-agent call's** prompt size (the window
  fill, not a sum) and `context.window` is the resolved model's `context_window`,
  or `null` when the entry declares none. A subagent's prompt is never reported
  here: it has its own, much smaller, window.
* An unknown session id is `404 not_found`.

`UsageTotals` is
`{ "calls", "input_tokens", "output_tokens", "cached_tokens",
   "cached_known_calls", "cache_hit_ratio", "last_input_tokens" }`.
`cache_hit_ratio` is `null` when **no** call reported a prefix-cache figure
(unknown, which is not 0%); `cached_known_calls` is the denominator. A reported
`0` is a real miss and stays `0.0`.

### Rewind

Rewinding sends a conversation back to an earlier message. Everything after it
stops being part of the conversation, and the files the agent changed since then
can optionally be put back.

`GET /api/sessions/:id/rewind?message=<id>` → preview. Read-only; it hides
nothing and writes nothing.
```json
{ "session_id": "s_ab12", "running": false,
  "point": { "session_id": "s_ab12", "user_message_id": 412,
             "prompt": "add a login page", "keep_through": 412,
             "before_prompt": 411, "messages_removed": 6 },
  "preview": { "session_id": "s_ab12", "message_id": 412,
               "messages_removed": 6, "incomplete": false,
               "opaque_tools": ["run_command"],
               "files": [ { "path": "src/auth.rs",
                            "restore": "…before-image bytes…",
                            "removes_file": false, "changes": 2,
                            "tools": ["write_file", "edit_file"],
                            "incomplete": false } ] } }
```
`message` is required and must name a message **in this session**; a foreign id
is `404 not_found` and a non-numeric one is `400 bad_request`. An assistant
message or a tool result is normalised to the user prompt that produced it, so
`point.user_message_id` is always a prompt. `keep_through` is the last message
that survives (the prompt); `before_prompt` is the message in front of it, used
by the modes that replace the prompt. `files` lists every path the journal
recorded in the rewound region: `restore` is the bytes the file is put back to,
`removes_file` is true when the agent created it, and `incomplete` marks a path
whose before-image was never captured (over 4 MiB, or not valid UTF-8).
`opaque_tools` names the calls a revert cannot reach.

`POST /api/sessions/:id/rewind` → the rewind, performed.
```json
{ "message_id": 412, "mode": "retry", "revert": true, "note": "optional" }
```
| Field | Required | Meaning |
| --- | --- | --- |
| `message_id` | yes | The message to rewind to. |
| `mode` | no | `none` (default) hides the tail and stops. `retry` re-sends the original prompt. `note` sends `note` instead. |
| `revert` | no | `false` by default. `true` restores the files the journal recorded before each change, newest first, then retires those journal rows. |
| `note` | for `note` | The replacement prompt. Also overrides the prompt for `retry`. An empty note is `400 bad_request`. |
| `keep_prompt` | no | Whether the chosen prompt survives. Defaults to `true` for `none` and `false` for `retry`/`note` - a replacement prompt must not follow the prompt it replaces. |

```json
{ "session_id": "s_ab12", "mode": "retry",
  "point": { … }, "preview": { … },
  "outcomes": ["restored", "removed"],
  "run_id": "r_9f3", "files_restored": 2, "files_skipped": 0 }
```
`outcomes` has one entry per journal row, newest first: `restored`, `removed`,
`not_captured` or `failed`. `run_id` is set by `retry` and `note` and is `null`
for `none`. `files_restored` counts the clean outcomes and `files_skipped` the
rest.

Rules:

* **Messages are hidden, not deleted.** The rows gain a `rewound_at` stamp and
  every transcript read filters them, so the model, the session detail and the
  message count agree. Nothing is recoverable-and-gone: the rows are still in
  SQLite and retention deletes them like any other old row.
* A session with a run in flight is `409 bad_request`; cancel it first. A live
  loop holds the conversation in memory and would write back what was hidden.
* `revert` restores only files journaled by `write_file` and `edit_file`. A
  `run_command`, a plugin tool or an MCP tool can change anything and journal
  nothing; the preview lists it in `opaque_tools` so a client can say so.
* `session_rewound` is emitted on `/api/events` for every client; a client should
  refetch the session rather than splice the transcript, because the server owns
  the normalisation and the journal's effects are not in the transcript at all.
* An unknown `mode` or an unknown body field is `400 bad_request`; neither is
  silently ignored.

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

#### Steering a run, following up, and recalling

`?mode=` decides what a message does to the run that may or may not be in flight.
`send` (the default) starts a run; the other two require one, and answer `409`
when there is nothing to queue against.

| `mode` | Effect | Accepted while |
| --- | --- | --- |
| `send` (default) | start a run | idle |
| `steer` | fold the message into the conversation at the running loop's next iteration | running |
| `follow_up` (or `follow-up`) | start a new run once the current one finishes | running |

`POST /api/sessions/:id/messages?mode=steer` (or `follow_up`)
```json
{ "content": "summarise\n\nAttached 1 file:\n\n### notes.md (inlined, 812 B)\n\n```md\n# Note\n```",
  "draft": "summarise",
  "attachments": [ { "name": "notes.md", "path": "notes.md", "mode": "inline",
                     "size": 812, "content": "# Note\n" } ] }
```
→ `202 { "mode": "steer", "session_id": "s_ab12", "steer_id": "steer_…",
         "queued": true, "queued_message": QueuedMessage }`
(`follow_up` answers with `follow_up_id` instead of `steer_id`.)

`content` is what the model will see. `draft` and `attachments` are what a
**recall** puts back on screen, so recalling a message carrying a 40 KB inlined
file does not paste that file into the textarea. `mode` defaults to `upload`;
`inline` takes an optional `content` with the inlined text, which is the only copy
of a file the browser read from disk (an `upload` may not send `content`: its
bytes are at `path`). A queued message carries at most 16 attachments, an inline
body is capped at 128 KiB, and a nameless or pathless attachment is
`400 bad_request`.

`GET /api/sessions/:id/queues` → both queues without the transcript:
```json
{ "session_id": "s_ab12", "running": true,
  "steers": [QueuedMessage], "follow_ups": [QueuedMessage] }
```
The same pair `GET /api/sessions/:id` carries as `queued_steers` and
`queued_follow_ups`, so a reload (or a second browser) shows the pending work
instead of losing it.

`DELETE /api/sessions/:id/queues/:queued_id`
→ `{ "session_id": "s_ab12", "recalled": QueuedMessage }`

Takes a queued message back out; it never reaches the model. An id that is not
queued - it already landed, or another client recalled it first - is
`404 not_found`, and the client is expected to refetch rather than revive a
message the run has applied.

`QueuedMessage`
```json
{ "id": "steer_7f2c…", "kind": "steer", "content": "…the rendered body…",
  "draft": "summarise", "accepted_at": 1710000000000,
  "attachments": [ { "name": "notes.md", "path": "notes.md", "mode": "inline",
                     "size": 812, "content": "# Note\n" } ] }
```
`kind` ∈ `steer|follow_up`. `draft` always carries the text to put back in the
composer (it equals `content` when nothing is attached), and `attachments` is
absent when there are none. Nothing here is persisted: a queued message exists
only until the run applies it, which is what makes one that never lands simply
absent from the transcript rather than a dangling turn.

Every queue mutation — accepted, recalled, drained into the conversation, taken
as a follow-up, or dropped with the session — publishes a `queued_messages` event
with the whole pair of lists, so a client that missed a frame cannot drift.

`POST /api/sessions/:id/cancel` → `{ "cancelled": true }` — `false` when no run is
active (already finished or never started).

### Shapes

`StoredMessage`
```json
{ "id": 42, "session_id": "s_ab12", "run_id": "r_x", "agent_id": null,
  "role": "assistant", "content": "...", "tool_calls": [ { "id": "call_1",
  "name": "read_file", "arguments": "{\"path\":\"a\"}" } ],
  "tool_call_id": null, "name": null, "created_at": 1710000000000,
  "images": [ { "media_type": "image/png",
                "data": "iVBORw0KGgo...",
                "bytes": 48213,
                "name": "assets/shot.png" } ],
  "reasoning_content": "..." }
```
`role` ∈ `system|user|assistant|tool`.

`images` is present only on a `tool` row whose tool attached an image for a
vision model (`read_file` on a PNG/JPEG/GIF/WebP). `data` is **standard base64**
with no `data:` prefix; the UI builds `data:${media_type};base64,${data}` for the
preview. The same list is sent live on the `tool_call_finished` event, so a chat
shows the image without refetching the transcript.

`reasoning_content` is present on an assistant row when the provider streams its
analysis separately. The text is stored with the turn and sent back on later
provider requests. Older rows and models without a separate reasoning stream omit
the field.

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

## Subagents

Subagent messages are stored in the parent session with `agent_id` set, and the
session transcript in the web UI deliberately hides them. These two routes expose
the delegated work itself.

`GET /api/agents?project=<id>&session=<id>&status=running,failed`
```json
{ "running": 2,
  "agents": [ { "id": "a_x", "session_id": "s_ab12", "parent_run_id": "r_x",
                "parent_agent_id": null, "name": "reviewer", "role": "reviewer",
                "task": "Review the diff", "status": "running", "model": "gpt-4o-mini",
                "iterations": 3, "summary": "", "error": null,
                "owned_paths": ["crates/foo/**"], "created_at": 0, "updated_at": 0,
                "finished_at": null, "session_title": "Review the harness",
                "workspace": "/home/me/project", "project_id": "p_ab12" } ] }
```
* Agents are returned newest first, across every session in scope. `project`
  narrows to one project; without it the list covers the whole server. `session`
  narrows to one session and `status` accepts a comma-separated set of
  `running|completed|failed|cancelled|interrupted`. An unknown status is
  `400 bad_request`.
* `running` is the number of agents with status `running` in the (filtered) list.
* Each record is an `AgentRecord` with the session's `session_title`, `workspace`
  and `project_id` flattened onto it, so a workspace-wide view needs one request,
  not one per session.

`GET /api/agents/:id?messages=N` → one agent and the transcript it wrote:
```json
{ "agent": AgentSummary, "messages": [StoredMessage],
  "messages_truncated": false, "messages_limit": 2000, "messages_before": null }
```
`messages` contains only rows whose `agent_id` matches, oldest first, tail-limited
by `?messages=` (default 2000, max 50000). It is the same `StoredMessage` shape the
session transcript uses, so the task turn, assistant text, thinking, tool calls and
tool results are all present. An unknown id is `404 not_found`.

Live activity arrives on the normal event stream, stamped with the agent's
`agent_id`: `text_delta`, `reasoning_delta`, `tool_call_started/finished`,
`command_started/output/finished`, `approval_requested/resolved`, plus the
lifecycle events `agent_spawned`, `agent_progress` and `agent_finished`. One
connection covers every agent in every session; there is no per-agent stream.

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

"Outside the workspace" means outside the project directory **and** outside the
system temporary directory, which every workspace also allows
(`allow_temp_dir`, on by default; `KARUCODE_ALLOW_TEMP_DIR=0` restricts it to the
project). The temp dir is a second root, not a widened project: `..` out of it
and a symlink inside it that points elsewhere are still `400 path_escape`.

`binary` is a NUL-byte sniff over the first 8000 bytes, so it is `false` for a
PDF or an SVG — both are ASCII. Treat the extension, not this flag, as the
authority on how to display a file.

### Saving a file

`PUT /api/workspace/file?path=docs/guide.md[&create=true][&force=true][&project=<id>]`
takes the **whole** new contents and answers `200`:

```bash
curl -s -X PUT "localhost:8787/api/workspace/file?path=docs/guide.md" \
  -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
  -d '{"content":"# New title\n","expected_size":412}'
```
```json
{ "path": "docs/guide.md", "size": 13, "created": false, "language": "markdown" }
```

Body: `{ "content": "<whole file>", "expected_size": <bytes, optional> }`.

* `content` is UTF-8 text only, at most **2 MiB** (`413 payload_too_large`,
  the same cap as the `write_file` tool). This endpoint never writes binary.
* The path is confined exactly like the read: `..`, absolute paths outside the
  workspace and symlink escapes are `400 path_escape`.
* A missing file is `404 not_found`. `create=true` creates it (and its parent
  directories) instead, and the response says `"created": true`.
* `expected_size` is optimistic concurrency: when the file on disk is no longer
  that size the save is `409` with the documented `bad_request` code and
  **nothing is written**, so an editor cannot clobber what the agent wrote in the
  meantime. `force=true` skips the check — it is the user's answer to that
  prompt, not a default. A file that was deleted while it was open is the same
  `409`, because saving would reverse somebody's deletion.
* `approvals.mode = "read_only"` refuses every save with `403
  permission_denied`. This is a user action, not an agent one, so it never
  creates an approval request.
* `POST`/`PATCH`/`DELETE` on this path is `405`; only `GET` and `PUT` exist.

### Raw bytes (image and PDF previews)

`GET /api/workspace/raw?path=assets/logo.png[&project=<id>]` returns the file's
bytes with a guessed `Content-Type`, `Content-Disposition: inline` and
`Cache-Control: no-store`:

```bash
curl -s -D- -o out.png "localhost:8787/api/workspace/raw?path=assets/logo.png"
# 200, content-type: image/png, content-disposition: inline; filename="logo.png"
```

`GET /api/workspace/file` answers JSON, which an `<img>` or an embedded PDF
cannot render, so the viewer points those elements here. The limit is **32 MiB**
(`413 payload_too_large` above it), confinement and error codes are the same as
the read endpoint, and `X-Content-Type-Options: nosniff` is always sent so a
workspace HTML file served here cannot be sniffed into a script.

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

## Chat artifacts

A reply can carry a fenced block the web UI draws as live content: an HTML mock, a
diagram, a table. The transcript renders it in a card with a Preview/Source
switch, and an HTML one is a **real document** in an `<iframe>`. These two routes
exist only to give that frame something to load; what an agent writes in the
fence (and what renders) is the `artifacts` topic in `karucode_docs`.

`POST /api/artifacts` → `201`:

```bash
curl -s -X POST localhost:8787/api/artifacts \
  -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
  -d '{"html":"<button>Buy</button>","theme":"dark"}'
```
```json
{ "id": "9f2c6d1b8a4e0f3c5d7b9a1e2c4f6d80",
  "url": "/api/artifacts/9f2c6d1b8a4e0f3c5d7b9a1e2c4f6d80",
  "bytes": 374, "theme": "dark" }
```

* `html` is the artifact source, at most **1 MiB** (`413 payload_too_large` above
  it; an empty or whitespace-only body is `400 bad_request`).
* `theme` is `light` (default) or `dark` and only decides the wrapper's own
  background and text colour, for a **fragment**. A complete document (`<!doctype
  html>` / `<html>`) keeps its own CSS and ignores `theme` — which is why the same
  page in either theme is one id.
* The id is the first 128 bits of the SHA-256 of the bytes actually served, so
  publishing the same artifact again is the same entry and the same id. The store
  is **in memory only**, bounded (128 entries / 16 MiB, oldest evicted first) and
  dropped when the process exits: a `404` simply means "post it again".

`GET /api/artifacts/<id>` → `200 text/html; charset=utf-8`:

* The document is served with a route-local policy: **sandboxed** into an opaque
  origin (`sandbox allow-scripts`, never `allow-same-origin`), inline `<script>`
  and `<style>` allowed, and **no network** (`connect-src 'none'`, `img-src data:
  blob:`, `font-src data:`, no wildcard source). A prompt-injected
  `<img src="https://…">` cannot load, so it cannot beacon out.
* `frame-ancestors 'self'` and `Cross-Origin-Resource-Policy: cross-origin` on this
  route only let this app embed it (and let the document's own inline assets load);
  `X-Frame-Options` is deliberately absent, because `DENY` would block the frame.
* `Cache-Control: no-store`. A malformed or unknown id is `404 not_found`, never a
  `5xx`.
* With `KARUCODE_AUTH_TOKEN` set this path also accepts `?token=<token>`: an
  `<iframe src>` is a browser navigation and cannot send `Authorization`. The
  token in the URL grants nothing the header did not - it is the same trade the
  event stream and the terminal socket already make.

## Git

The git routes are the source-control manager's backend. They are **local only**:
staging, commits, branches, history and stashes. There is deliberately no
fetch/pull/push route - those need credentials and can block on a prompt, so they
stay in the terminal.

`GET /api/git/status[?project=<id>]` →
```json
{ "is_repo": true, "branch": "main", "detached": false,
  "upstream": "origin/main", "ahead": 1, "behind": 0, "error": null,
  "truncated": false,
  "files": [ { "path": "src/x.rs", "orig_path": null,
               "index": "M", "worktree": " ", "status": "M",
               "staged": true, "unstaged": false,
               "untracked": false, "conflict": false } ] }
```
`index`/`worktree` are the two porcelain columns, so a path staged *and* modified
(`MM`) belongs to both the staged and the working-tree list; `status` is the
trimmed form older clients read. `detached: true` means `branch` is `null`.
`truncated: true` means the listing hit the server's output cap.

`GET /api/git/diff?path=&staged=false&base=[&project=<id>]` →
```json
{ "diff": "diff --git ...", "truncated": false }
```
An untracked path still renders: it is compared against the empty file, so a new
file reads as added lines instead of an empty patch.

Reads for the manager:

* `GET /api/git/branches` → `{ is_repo, branch, detached, truncated, branches: [{
  name, current, commit, upstream, ahead, behind, timestamp, subject }] }`
* `GET /api/git/log[?limit=30&path=src/x.rs]` → `{ is_repo, truncated, commits: [{
  id, short_id, author, email, timestamp, parents, refs, subject }] }` (`limit` is
  clamped to 1-100; `path` narrows the log to one file's history)
* `GET /api/git/commits/:sha` → the commit with `body`, a `files` stat
  (`{path, additions, deletions, binary}`) and its `diff`; `404 not_found` for a
  well-formed id that is not in the repository, `400` for a non-hex one
* `GET /api/git/stash` → `{ is_repo, truncated, stashes: [{ ref, index, commit,
  timestamp, subject }] }`

Every mutation answers the same envelope, with a **fresh** status so a client
never races a second `GET` against a command it just ran:

```json
{ "ok": true, "message": "switched to main", "status": { … }, "commit": null }
```

`commit` carries `{id, short_id, subject}` after `POST /api/git/commit` and is
`null` everywhere else.

| Route | Body |
| --- | --- |
| `POST /api/git/stage`, `/unstage` | `{ "paths": ["src/x.rs"] }` or `{ "all": true }` |
| `POST /api/git/discard` | the same plus `"staged": bool` and **required** `"confirm": true` |
| `POST /api/git/commit` | `{ "message": "fix: x", "amend": false, "stage_all": false }` |
| `POST /api/git/branches` | `{ "name": "feat/y", "start": "main", "checkout": true }` |
| `POST /api/git/branches/delete` | `{ "name": "feat/y", "force": false, "confirm": false }` |
| `POST /api/git/checkout` | `{ "name": "main" }` |
| `POST /api/git/stash` | `{ "action": "push\|pop\|apply\|drop", "index": 0, "message": "…", "include_untracked": false, "confirm": false }` |

Rules:

* `discard` deletes an untracked path from disk, and `stash drop` deletes the
  stash entry; both require `confirm: true`, as does a forced branch delete
  (`force` + `confirm`). None of the three can be undone from the UI.
* `discard` with `staged: true` restores the index *and* the working tree to
  `HEAD`; without it, the working tree is restored from the index.
* `commit` with an empty message and `amend: true` keeps the previous message
  (`--no-edit`). An empty message without `amend` is `400 bad_request`.
* Every path is resolved inside the workspace before git sees it (`..`, an
  absolute path outside it and a symlink escape are refused), and the pathspec is
  passed after `--` with `GIT_LITERAL_PATHSPECS=1`, so a glob cannot match more
  than the caller named. Branch names and refs are validated by
  `kc_tools::git::validate_branch_name`/`validate_revision_arg`; a commit id must
  be hex.
* A refused command is `400 bad_request` carrying git's own message (nothing to
  commit, a dirty checkout, an unmerged branch), so it can be shown verbatim.
* Reads are bounded at 10 s and mutations at 60 s; `git` runs with the shared
  `SAFE_CONFIG` and environment, so repository config cannot execute anything on
  a read. Mutations do run the repository's own hooks and clean/smudge filters,
  exactly like a terminal `git` command.
* A commit needs a git identity (`user.name`/`user.email` in the repository or the
  service account's `~/.gitconfig`); karucode never invents one.

Both `status` and `diff` take the project-scoped query parameter described above;
`git` is run with `current_dir` set to that project's directory. `status` reports
failures in the payload (`200` with `is_repo: false` and `error` set) including a
missing `git` binary; `diff` answers `400 bad_request` when the directory is not a
git repository and `500 internal` when `git` cannot be spawned.

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

`POST /api/plugins/reload` → `{ "report": PluginRescanReport, "plugins":
[PluginDiagnostics] }`

Re-reads every plugin source — the `[[plugins]]` entries in the config file plus
`<data_dir>/plugins/*/plugin.toml` — and reconciles the running processes with it.
No body. The report says what happened, in discovery order:

```json
{ "added": ["wordy"], "removed": ["old"], "restarted": [],
  "unchanged": ["example-hello"] }
```

* `added` — a new name: process started, tools exposed.
* `removed` — the name is gone from every source: process stopped, tools dropped.
* `restarted` — same name, different config entry, manifest or **file**: the old
  process was stopped and a new one started. A rebuilt binary counts, because
every command argument that resolves to a file is fingerprinted (`mtime` + size).
* `unchanged` — identical sources: the process was not touched.

The tool registry is rebuilt before the response, so the model sees the new set on
its next call; a run already in flight keeps the snapshot it started with. A
runtime `enabled` toggle survives a rescan.

`POST /api/plugins/:name/reload` → `PluginDiagnostics`

Replaces one plugin's process (the "I rebuilt it" / "I edited its manifest" path).
No body. A plugin that is disabled at runtime is rebuilt but not started. Errors:
unknown name → `404 not_found`; a slot discovery already rejected → `400
bad_request`. A plugin that fails to start is `200` with `status: "failed"` and
the reason in `error`, exactly like `/enable`.

With `plugins_watch = true` in the config file the server calls the rescan route's
code path on a timer (`plugins_watch_secs`, clamped to 1..=600) whenever its plugin
sources move, so none of this needs to be triggered by hand.

`plugin_panels` in `GET /api/config` lists the inspector tabs the ready plugins
declare, grouped per plugin. A panel is `{"id","hosted_id","title","icon",
"source","refresh_ms"}` where `hosted_id` is `plugin__<plugin>__<id>`. A plugin
that declares none contributes no entry and no error; the field is absent only on
an older server.

`GET /api/plugins/:name/panels/:panel[?project=<id>&session=<id>]` →
```json
{ "title": "Todo", "data": { "items": [] },
  "version": 1793285052052, "fetched_ms": 1793285052052 }
```
`data` is **opaque JSON** produced by the plugin and never interpreted here: the
browser renders it with a component registered for that panel type, or as
read-only JSON, so a plugin cannot inject markup or script. `title` falls back to
the manifest's; `version` equals `fetched_ms` and is a cache key. `?project=`
selects the workspace root and `?session=` the session handed to the plugin.
Errors: unknown plugin or panel → `404 not_found` (the message lists the panels
the plugin does declare); disabled or unusable plugin → `400 bad_request`; a
plugin that fails to answer → `502 plugin`.

`GET /api/mcp` → `{ "servers": [ { "name": "fs", "transport": "stdio",
  "status": "connected|failed|disconnected", "tools": ["mcp__fs__read"],
  "error": null } ] }`

## Event stream

`GET /api/events?session=<optional session id>` — `text/event-stream`.
Use `fetch()` plus a `ReadableStream` reader (not `EventSource`, so the auth
header travels). Each event is:

```
id: 12
data: {"seq":12,"ts_ms":1710000000123,"session_id":"s_x","run_id":"r_y","agent_id":null,"type":"text_delta","delta":"Hello"}\n\n
```

A `: keepalive` comment is sent every 15 seconds. The envelope always has
`seq`, `ts_ms`, and optional `session_id`, `run_id`, `agent_id`; the event fields
are flattened alongside `type`.

**Resume.** Every frame carries `id: {seq}` before its `data:` line, so a
reconnect can send `Last-Event-ID: <seq>` (or `?last_event_id=<seq>`) and be
handed the frames it missed instead of refetching the session. The server keeps
the last 1000 envelopes per session in memory; a cursor older than that gets a
`resync` frame (`{"type":"resync","requested_id":N,"oldest_available":M}`) rather
than a silent skip, and the client should refetch once.

When a resume is not possible, the client has to notice what it missed:

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
| `session_rewound` | `session_id`, `message_id` (the prompt the rewind normalised to), `messages_removed` (`0` = no-op, e.g. a retry of the newest prompt). Refetch the session rather than splicing it: the server owns the normalisation and the file journal's effects are not in the transcript |
| `steer_applied` | `steer_id`, `content` — a queued steer joined the conversation. It is now a user turn in the transcript, so drop it from the pending list |
| `queued_messages` | `session_id`, `steers`, `follow_ups` — the whole pair of queues, published on every mutation (accepted, recalled, drained, taken as a follow-up, dropped with the session). Replace the local pending list with what the frame says: the newest frame is the truth, so a missed one cannot leave it stale |
| `run_started` | `run_id`, `prompt` |
| `run_finished` | `run_id`, `status`, `error` |
| `run_cancelled` | `run_id` |
| `iteration_started` | `iteration` |
| `stream_retry` | `attempt`, `max_attempts`, `reason` — the provider stream died mid-answer; everything streamed for this turn so far belongs to the abandoned attempt and must be dropped (see `retry_max_stream_restarts`) |
| `text_delta` | `delta` |
| `reasoning_delta` | `delta` |
| `usage` | `input_tokens`, `output_tokens` |
| `tool_call_started` | `call_id`, `name`, `arguments` (object; a **string** containing the raw text when the model emitted invalid JSON) |
| `tool_call_finished` | `call_id`, `name`, `ok`, `output`, `truncated`, `duration_ms`, `images?` (`[{media_type, data, bytes, name}]`, standard base64 — present only when the tool attached an image for a vision model) |
| `command_started` | `call_id`, `command`, `cwd` |
| `command_output` | `call_id`, `stream` (`stdout`/`stderr`), `chunk` |
| `command_finished` | `call_id`, `exit_code`, `duration_ms`, `truncated` |
| `approval_requested` | `approval` (ApprovalRecord shape + payload) |
| `approval_resolved` | `approval_id`, `decision` |
| `agent_spawned` | `agent_id`, `name`, `role`, `task`, `model`, `parent_run_id` |
| `agent_progress` | `agent_id`, `iterations`, `note` |
| `agent_finished` | `agent_id`, `status`, `summary`, `error` |
| `notice` | `level`, `message` |
| `notification` | `notification` — a `{id, kind, title, body, session_id, session_title, project_id?, source, sound?, sticky}` payload. `kind` is `run_finished`, `needs_input`, `error` or `info`; `source` is `runtime` or a plugin's name. `project_id` is the project the session belongs to and is **omitted** when the notice names no session or its session could not be read — the browser uses it to count unread news per project tile, since it only ever lists the open project's sessions. Emitted after the state it describes is persisted, and **not emitted at all** when `[notifications] enabled = false`. Render an unknown `kind` as `info` rather than dropping it. |
| `stream_lagged` | `skipped` - the SSE pump dropped frames for this client; refetch the session (emitted by the server, not the runtime) |
| `error` | `message`, `kind` |

## Frontend build/dev

Vite dev server proxies `/api` to `KARUCODE_PORT` (default `8787`). In production
the Rust server serves the built `web/dist` (embedded at compile time, or from
`KARUCODE_WEB_DIR` when set).

## Metrics

`GET /metrics` — Prometheus text format (`text/plain; version=0.0.4`).

This is an operator endpoint, not part of the `/api` surface, and it sits **inside**
the auth layer: with `KARUCODE_AUTH_TOKEN` set, scraping it needs the bearer token.
It names routes and counts, which is not something to hand to an unauthenticated
caller on a non-loopback bind.

Series include `karucode_requests_total{method,status_class}`,
`karucode_sse_connections`, `karucode_runs_active`, `karucode_runs_started_total`,
`karucode_runs_finished_total` and `karucode_uptime_seconds`.

Every response also carries an `X-Request-Id` header (a fresh v4 id per request),
and the same id appears in the access-log line for that request, so a log entry and
a client report can be matched. The access log records method, path, status and
latency — never the query string (which can carry `?token=`) and never a body.

Post-start mission replanning: `POST /api/missions/:id/tasks/:task_id/replan`
requires an explicit user action and JSON `expected: {project_id, prompt,
dependencies}` plus at least one replacement field among `project_id`, `prompt`,
and `dependencies`. It returns the updated task; stale or ineligible changes
return 400. Only never-claimed pending/blocked tasks in active or paused missions
are eligible. The scheduler may claim the task immediately after approval.

### Central control chat

`POST /api/control/session` reserves or reopens one persistent central chat and
returns `{session_id, project_id, created}`. It requires a registered project;
the first reservation chooses one as its workspace and retains that binding.
Use the ordinary session transcript, message, event and approval APIs. The chat
is not an isolation boundary: its normal tools operate in its bound workspace.

Only that chat's root agent can call `control_start_session`, with a unique
`request_id`, explicit registered `project_id`, `title`, and `prompt`. The
approval presents the target project, path and complete prompt. `read_only`
refuses, `ask` and `ask_commands` ask, and `auto` permits the dispatch. A
transaction reserves the request and creates an ordinary project-local session;
reusing the key with different input is rejected and identical replays never
start another run. Reservation and runtime submission are not atomic: after a
crash or failed submission inspect the returned session rather than generating
a new request key. Deleting the control chat leaves its reservation intact and
cannot silently create a replacement. This mechanism does not change missions.
