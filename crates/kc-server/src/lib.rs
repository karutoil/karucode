//! HTTP API, SSE event stream and static frontend serving.
//!
//! The binary is a thin wrapper around [`run`]. Everything testable lives in
//! modules: [`settings`] (layered config), [`state`] (wiring types), [`app`]
//! (router + security middleware), [`routes`] (handlers), [`static_ui`] (asset
//! serving) and [`bootstrap`] (process startup/shutdown).
//!
//! # Router construction
//!
//! ```no_run
//! # async fn example(state: kc_server::state::AppState) {
//! let router = kc_server::build_router(state);
//! # let _ = router;
//! # }
//! ```
//!
//! `Router::new().nest("/api", routes::api_router()).fallback(static_ui::serve_spa)`
//! with four layers, outermost first: security headers, bearer auth, the
//! `Host` guard (DNS rebinding, see [`host`]) and CSRF.

// @kc-doc: http-api
// @kc-title: Call the HTTP API
// @kc-summary: Authenticate, send the CSRF header, and find the endpoint for sessions, config, approvals, files, git, plugins, MCP and the event stream.
// @kc-tags: api, http, rest, sse, curl, endpoints
// @kc-applies-to: route:GET /api/health, route:GET /api/config, route:PATCH /api/config, route:POST /api/provider/test, route:GET /api/models/catalog, route:GET /api/models/lookup, route:GET /api/stats, route:GET /api/projects, route:POST /api/projects, route:GET /api/projects/:id/favicon, route:GET /api/sessions, route:POST /api/sessions, route:GET /api/sessions/:id, route:GET /api/sessions/:id/info, route:PATCH /api/sessions/:id, route:PATCH /api/sessions/:id/model, route:POST /api/sessions/:id/messages, route:GET /api/sessions/:id/queues, route:DELETE /api/sessions/:id/queues/:queued_id, route:POST /api/sessions/:id/cancel, route:GET /api/sessions/:id/rewind, route:POST /api/sessions/:id/rewind, route:GET /api/agents, route:GET /api/agents/:id, route:GET /api/approvals, route:POST /api/approvals/:id, route:GET /api/workspace/tree, route:GET /api/workspace/file, route:PUT /api/workspace/file, route:GET /api/workspace/raw, route:POST /api/workspace/upload, route:GET /api/git/status, route:GET /api/git/diff, route:GET /api/plugins, route:POST /api/plugins/reload, route:POST /api/plugins/:name/reload, route:POST /api/plugins/:name/enable, route:GET /api/plugins/:name/panels/:panel, route:GET /api/mcp, config:plugin_panels, route:GET /api/events, event:notification, event:queued_messages, event:session_rewound, config:[notifications], header:X-Karucode-Client, env:KARUCODE_AUTH_TOKEN, doc:docs/API.md
//
// The API is same-origin only; there is no CORS surface. Two rules matter:
//
// 1. **Auth** - with `KARUCODE_AUTH_TOKEN` set, every request needs
//    `Authorization: Bearer <token>`. `GET /api/events` also accepts
//    `?token=` because a `ReadableStream` fetch cannot always set headers.
// 2. **CSRF** - every `POST`/`PATCH`/`PUT`/`DELETE` must send
//    `X-Karucode-Client: web`, and a present `Origin` must match `Host`.
//
// ```bash
// curl -s localhost:8787/api/health | jq
// curl -s localhost:8787/api/sessions | jq
// curl -s -X POST localhost:8787/api/sessions/abc/messages \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"content":"run the tests and fix the failure"}'
// ```
//
// # Endpoint groups
//
// | Path | Purpose |
// | --- | --- |
// | `GET /api/health`, `/api/stats` | liveness, version, tool and process counts |
// | `GET`/`PATCH /api/config` | effective settings; validated, hot-swapped patches |
// | `POST /api/provider/test` | live probe of a profile, base_url or model |
// | `GET /api/models/catalog`, `/api/models/lookup` | models.dev metadata, cached |
// | `GET`/`POST /api/projects` | list and create projects; `GET /api/projects/:id/favicon` serves the project's own `favicon.ico` as `image/*` (`204` when it has none) |
// | `GET`/`POST /api/sessions` | list, create, load sessions |
// | `GET /api/sessions/:id` | one session with its transcript (`?messages=N`, default 5000, max 50000) |
// | `GET /api/sessions/:id/info` | the session's counts and token totals: messages, turns, tool calls, working time, per-agent usage, window fill |
// | `PATCH /api/sessions/:id`, `PATCH /api/sessions/:id/model` | rename, and pin or clear this session's model |
// | `POST /api/sessions/:id/messages` | start a run (SSE progress arrives on `/api/events`); `?mode=steer` folds the message into the run in flight, `?mode=follow_up` starts one after it |
// | `GET /api/sessions/:id/queues`, `DELETE /api/sessions/:id/queues/:queued_id` | the messages waiting for the run, and recall one (it never reaches the model) |
// | `POST /api/sessions/:id/cancel` | cancel the active run |
// | `GET`/`POST /api/sessions/:id/rewind` | preview, then hide the transcript after a message (optionally reverting its file changes, retrying it, or replacing it with a note) |
// | `GET /api/agents`, `GET /api/agents/:id` | every subagent in a project, and one agent's own transcript |
// | `GET /api/approvals`, `POST /api/approvals/:id` | list and answer pending approvals |
// | `GET /api/workspace/tree`, `/api/workspace/file` | workspace explorer, confined to the project |
// | `PUT /api/workspace/file`, `GET /api/workspace/raw` | save a file from the editor, and serve its raw bytes for image/PDF previews |
// | `POST /api/workspace/upload` | attach a file for the composer |
// | `GET /api/git/status`, `/api/git/diff` | repository state |
// | `GET /api/plugins`, `POST /api/plugins/:name/enable` | plugin status, enable/disable |
// | `POST /api/plugins/reload` | rescan every plugin source: start new plugins, drop removed ones, replace changed ones (no server restart) |
// | `POST /api/plugins/:name/reload` | replace one plugin's process (after a rebuild or a manifest edit) |
// | `GET /api/plugins/:name/panels/:panel` | one plugin panel's payload for the inspector |
// | `GET /api/mcp` | MCP server status and discovered tools |
// | `GET /api/events` | the SSE event stream for the open session |
//
// # Reading a long transcript
//
// A session with thousands of tool calls has far more messages than one screen.
// `GET /api/sessions/:id` returns the **newest** `messages` rows (default 5000,
// `?messages=N` up to 50000) and two fields that make the cut explicit instead
// of silent:
//
// ```json
// { "messages": [ ... ], "messages_limit": 5000, "messages_truncated": true }
// ```
//
// `messages_truncated: true` means older messages exist in the database and a
// larger `?messages=` (or a client-side "load older") will fetch them; nothing
// is deleted. A non-numeric `messages` is `400 bad_request`.
//
// # Per-session model selection
//
// Each session can run a different provider + model. The install-wide
// `active.provider` / `active.model` in `GET /api/config` is the default for new
// and unpinned sessions; pinning one session never moves that default.
//
// ```bash
// # Pin one session to another profile/model, with optional reasoning effort.
// curl -s -X PATCH localhost:8787/api/sessions/abc/model \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"provider":"local","model":"qwen3-coder","reasoning_effort":"high"}'
//
// # Follow the install-wide default again.
// curl -s -X PATCH localhost:8787/api/sessions/abc/model \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' -d '{}'
// ```
//
// `PATCH /api/sessions/:id` accepts the same pin as `{"model": {...}}`, and
// `{"model": null}` clears it. The pin is validated against the live provider
// profiles (`400 bad_request` names the field), every change emits
// `session_model_changed` on `/api/events`, and a run resolves its model from the
// session, so concurrent sessions use different models. If the pinned profile or
// model is deleted later, the run falls back to the default instead of failing.
// Subagents inherit their session's pin. `reasoning_effort` accepts `none`,
// `minimal`, `low`, `medium`, `high` or `max` and is forwarded to the endpoint.
//
// # Rewinding a session
//
// Rewinding sends a conversation back to an earlier message: everything after it
// stops being part of the conversation, and optionally the files the agent
// changed since then are put back.
//
// ### Preview (read-only)
//
// ```bash
// curl -s "localhost:8787/api/sessions/s_ab12/rewind?message=412" | jq
// ```
//
// ```json
// { "session_id": "s_ab12",
//   "point": { "user_message_id": 412, "prompt": "add a login page",
//              "keep_through": 412, "before_prompt": 411, "messages_removed": 6 },
//   "preview": { "files": [ { "path": "src/auth.rs", "restore": "…",
//                             "removes_file": false, "changes": 2,
//                             "tools": ["write_file","edit_file"], "incomplete": false } ],
//                "incomplete": false, "opaque_tools": ["run_command"] },
//   "running": false }
// ```
//
// `message` is required and must be an id in *this* session. An assistant
// message is normalised to the user prompt that produced it - `point` always
// describes a prompt, so "rewind to this answer" and "rewind to the request
// behind it" are the same operation. The preview writes nothing; a client may
// call it on hover.
//
// ### Perform
//
// ```bash
// curl -s -X POST localhost:8787/api/sessions/s_ab12/rewind \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"message_id":412,"mode":"retry","revert":true}'
// ```
//
// | Field | Meaning |
// | --- | --- |
// | `message_id` | required; the message to rewind to |
// | `mode` | `none` (default) hides the tail and stops, `retry` re-sends the original prompt, `note` sends `note` instead |
// | `revert` | `false` by default. `true` restores the files the journal recorded before each change, newest first |
// | `note` | required for `mode: "note"`; also overrides the prompt for `retry` |
// | `keep_prompt` | defaults to `true` for `none` and `false` for `retry`/`note`, because a replacement prompt must not follow the one it replaces |
//
// The response carries `point`, `preview`, `outcomes` (`restored`, `removed`,
// `not_captured` or `failed`, one per journal row, newest first), `run_id` (set
// by `retry` and `note`), and `files_restored` / `files_skipped`. Every client
// on `/api/events` also receives `session_rewound`.
//
// ### What a rewind does and does not do
//
// * Messages are **hidden, not deleted**: `rewound_at` is stamped and every
//   transcript read filters it. A reload, the model's next request and the
//   session's message count all agree, and the rows are still in SQLite.
// * A running session is refused with `409` - a live loop holds the conversation
//   in memory and would write back what the rewind hid. Cancel first.
// * Files are only restored for tools that journal a before-image
//   (`write_file`, `edit_file`, and any future tool that does the same). A file
//   over 4 MiB, or one that is not valid UTF-8, is recorded as *not captured* and
//   reported as `incomplete` rather than half-restored.
// * `opaque_tools` names the calls a revert cannot reach - `run_command`, a
//   plugin, an MCP tool. Their effects stay; a client should say so instead of
//   implying a clean revert.
// * An unknown `mode` or an unknown field is `400`; neither is silently ignored.
//
// # Project scoping
//
// Session, workspace, git and stats routes accept `?project=<id>`. Without it they
// answer for the default workspace. `/api/health` and `/api/config` are
// install-wide.
//
// # Project icons
//
// `GET /api/projects/:id/favicon[?max_bytes=N]` answers `image/*` with the
// project's own `favicon.ico` - bytes, not JSON, so an `<img>`-shaped consumer
// can cache it and no base64 bloats `GET /api/projects`. When the project has no
// usable icon (the normal case, or the file is larger than `max_bytes`, default
// and maximum 512 KiB) it answers an empty `204 No Content` rather than a `404`:
// browsers log every 4xx as `Failed to load resource`, so a routine absence would
// look like a failure. An unknown project id is still `404 not_found`.
//
// The search rules - one file name, `.gitignore` honoured, shallowest match wins
// - and the rail's fallback to initials are in the `projects` topic.
//
// ```bash
// curl -s "localhost:8787/api/projects/p_ab12/favicon" -o icon.ico
// curl -s -o /dev/null -w '%{http_code}\n' "localhost:8787/api/projects/p_ab12/favicon"
// # 204 when the project ships none, 404 when the project id is unknown
// ```
//
// # Editing workspace files
//
// The web viewer reads with `GET /api/workspace/file`, saves with
// `PUT /api/workspace/file` and renders images/PDFs from
// `GET /api/workspace/raw`.
//
// ```bash
// # Save the whole file. `expected_size` is the size the editor loaded.
// curl -s -X PUT "localhost:8787/api/workspace/file?path=docs/guide.md" \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"content":"# New title\n","expected_size":412}'
// # 200 {"path":"docs/guide.md","size":13,"created":false,"language":"markdown"}
// ```
//
// The body is `{"content": "<whole file>", "expected_size": <bytes, optional>}`.
// It is UTF-8 text only (binary files have no editor) and at most 2 MiB, the same
// cap as the `write_file` tool (`413 payload_too_large`). Rules:
//
// * the path is confined exactly like the read: `..`, absolute paths outside the
//   workspace and symlink escapes are `400 path_escape`;
// * a missing file is `404 not_found`; pass `create=true` to deliberately create
//   one (parent directories are made);
// * `expected_size` mismatching the file on disk is `409 bad_request` and nothing
//   is written, so a save cannot silently clobber what the agent wrote in the
//   meantime. `force=true` overwrites anyway - it is the user's answer to that
//   prompt, not something to send by default;
// * `approvals.mode = "read_only"` refuses every save with `403
//   permission_denied`. The editor is a user action, so it never creates an
//   approval request: the modes that ask the agent are not consulted.
//
// `GET /api/workspace/raw?path=<file>[&project=<id>]` returns the bytes with a
// guessed `Content-Type`, `Content-Disposition: inline` and `Cache-Control:
// no-store`, up to 32 MiB (`413` above that). It exists because the JSON read
// endpoint cannot be an `<img src>` or an embedded PDF. The bytes are served as an
// inert asset: `X-Content-Type-Options: nosniff` is always on and the UI's CSP has
// no inline script, so a workspace HTML file served here cannot execute.
//
// Note that `binary` from the read endpoint is a NUL-byte sniff, and a PDF or an
// SVG is ASCII - so it is `false` for both. Decide how to display a file from its
// extension, not from that flag.
//
// # Plugin panels
//
// A plugin may contribute a tab to the inspector. `GET /api/config` lists what
// the ready plugins declare, grouped per plugin:
//
// ```json
// "plugin_panels": [
//   { "plugin": "kc-todo", "version": "0.2.0",
//     "panels": [ { "id": "todo", "hosted_id": "plugin__kc-todo__todo",
//                   "title": "Todo", "icon": "check", "source": "static",
//                   "refresh_ms": 0 } ] }
// ]
// ```
//
// The payload of one panel comes from `GET
// /api/plugins/:name/panels/:panel[?project=<id>&session=<id>]`:
//
// ```bash
// curl -s "localhost:8787/api/plugins/kc-todo/panels/todo?session=s_ab12" | jq
// # {"title":"Todo","data":{"items":[…]},"version":1793285052052,"fetched_ms":1793285052052}
// ```
//
// * `data` is **opaque JSON**: the server never interprets it, and the browser
//   renders it with a component registered for that panel or as read-only JSON.
//   A plugin cannot return markup or script, so the panel route is no more
//   privileged than `/api/config`.
// * `title` is the plugin's chosen label (the manifest's when it omits one);
//   `version` equals `fetched_ms` and is a cache key, not a data version.
// * `?project=` resolves the workspace the panel is read for and `?session=`
//   identifies the session; both are handed to the plugin. An unknown project or
//   session is a `404` — a panel is never rendered for a scope that does not
//   exist.
// * Errors follow the `enable` mapping: an unknown plugin or panel is
//   `404 not_found` (the message names the panels the plugin does declare), a
//   disabled or unusable plugin is `400 bad_request`, and a plugin that fails to
//   answer is `502 plugin` with its message.
//
// A plugin that declares no panels produces no `plugin_panels` entries and no
// error, so an older plugin needs no change.
//
// # Attachments
//
// `POST /api/workspace/upload?session=<id>&name=<file>[&project=<id>]` takes the
// raw file bytes and writes them to `.karucode/attachments/<session>/` inside the
// session's project, so the workspace-confined file tools can read them. Nothing
// is overwritten: a duplicate name becomes `report-1.pdf` and the response says
// `"deduplicated": true`.
//
// ```bash
// curl -s -X POST "localhost:8787/api/workspace/upload?session=s_ab12&name=report.pdf" \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/pdf' \
//   --data-binary @report.pdf
// # 201 {"session":"s_ab12","name":"report.pdf","path":".karucode/attachments/s_ab12/report.pdf", ...}
// ```
//
// The name is a single segment (no `/`, `..` or control characters), the session
// must exist, the body must be non-empty and at most 8 MiB (`413
// payload_too_large` otherwise), and form encodings are refused because they
// would be stored verbatim. `path` is workspace-relative - feed it back to
// `GET /api/workspace/file` or mention it in a prompt.
//
// # A session's numbers
//
// `GET /api/sessions/:id/info` answers "how much work was this, and where did
// the tokens go" without shipping a transcript. It is what the inspector's
// **Session** tab reads, and it is separate from `GET /api/sessions/:id` because
// that route exists to load a transcript: it carries up to 50 000 message rows,
// while this one carries counts and totals and stays cheap to poll on a long run.
//
// ```json
// { "session": Session, "running": false,
//   "stats": { "messages_total": 481, "messages_main": 402, "messages_subagent": 79,
//              "messages_by_role": {"user": 12, "assistant": 61, "tool": 408},
//              "messages_main_by_role": {"user": 9, "assistant": 40, "tool": 353},
//              "tool_calls_total": 408,
//              "tool_calls": {"read_file": 210, "run_command": 96, "edit_file": 74, ...},
//              "runs_by_status": {"completed": 11, "failed": 1}, "runs_total": 12,
//              "working_ms": 1_842_119 },
//   "usage": { "main": UsageTotals, "by_agent": {"a_x": UsageTotals},
//              "total": UsageTotals },
//   "context": { "model": "gpt-4o-mini", "window": 128000, "used": 41023 },
//   "agents": [ { "id": "a_x", "name": "reviewer", "role": "reviewer",
//                 "status": "completed", "model": "gpt-4o-mini", "iterations": 7,
//                 "duration_ms": 61234, "created_at": 0, "finished_at": 0,
//                 "messages": 79, "usage": UsageTotals } ],
//   "messages": { ... }, "tools": { "total": 408, "by_name": [{"name","count"}],
//                                   "truncated": false },
//   "runs": { "total": 12, "by_status": {...}, "working_ms": 1842119,
//             "recent": [RunRecord] } }
// ```
//
// What each part answers:
//
// * **messages** - whole-session counts, which a bounded transcript window cannot
//   give. `messages_main` is the conversation you see; `messages_subagent` is
//   everything delegated. Rewound rows are excluded, so the counts match the
//   transcript. A "turn" has no stored row of its own: it is a `user` row in
//   `messages_main_by_role`, and `runs_total` is the number of times the agent
//   actually looped over it.
// * **tools** - one entry per call, counted from the `tool_calls` JSON on each
//   assistant row, capped at 40 names (`truncated: true` when more exist). The
//   total always counts every call.
// * **runs.working_ms** - the sum of `finished_at - started_at` over runs that
//   finished. That is time the agent was *working*: idle gaps between runs are
//   not included, and an in-flight run is not counted until it ends (otherwise
//   the number would shrink when it finished).
// * **usage** - token totals **per agent**, from one persisted row per provider
//   call (see `UsageTotals` below). `main` is the run's own model calls;
//   `by_agent` is each subagent's; `total` is the session's whole spend and is
//   exactly main plus every agent. This survives a reload, which the `usage`
//   event cannot: that event is live-only.
// * **context** - how full the window is. `used` is the **newest main-agent
//   call's** prompt size (the fill), not a sum of every call (that is `usage`),
//   and `window` is the resolved model's `context_window` or `null` when the
//   entry does not declare one - the UI hides the percentage rather than
//   inventing a denominator. A subagent's prompt is never reported here: it has
//   its own, much smaller, window.
//
// `UsageTotals` is
// `{"calls","input_tokens","output_tokens","cached_tokens","cached_known_calls","cache_hit_ratio","last_input_tokens"}`.
// `cache_hit_ratio` is `null` when no call reported a prefix-cache figure -
// **unknown, which is not 0%** - and `cached_known_calls` is the denominator, so
// a client can say "no cache data" instead of drawing an empty bar. A reported
// `0` is a real miss and stays `0.0`.
//
// An unknown session id is `404 not_found`, not a page of zeros.
//
// ```bash
// curl -s localhost:8787/api/sessions/abc/info | jq '.usage.total, .context'
// ```
//
// # Inspecting subagents
//
// A subagent's messages are stored with `agent_id` set and are deliberately
// excluded from the session transcript the main chat renders, so these routes are
// how a client watches delegation without opening the database:
//
// * `GET /api/agents?project=<id>` lists every agent in the project, newest
//   first, with the session title/workspace flattened onto each record. Narrow it
//   with `&session=<id>` or `&status=running,failed` (comma-separated; an unknown
//   status is `400 bad_request`). The response also carries `running`, so a badge
//   does not have to count the array. Without `?project=` it lists every agent on
//   the server.
// * `GET /api/agents/:id` returns `{ agent, messages, messages_truncated,
//   messages_limit }`: only that agent's messages, oldest first, tail-limited by
//   `?messages=N` (default 2000, max 50000). This is the drill-in transcript.
//
// ```bash
// curl -s 'localhost:8787/api/agents?project=p_ab12&status=running' | jq '.running, .agents[].name'
// curl -s 'localhost:8787/api/agents/a_x?messages=200' | jq '.agent.status, (.messages | length)'
// ```
//
// The live event stream carries the same activity: `agent_spawned`,
// `agent_progress`, `agent_finished` for lifecycle, and the normal `text_delta`,
// `tool_call_started/finished`, `command_*` and `approval_*` frames stamped with
// the subagent's `agent_id`. One SSE connection therefore feeds every agent in
// every session; no per-agent stream exists.
//
// # The event stream
//
// `GET /api/events?session=<id>` is `text/event-stream`: message deltas, tool calls
// and results, approval requests, agent progress, run completion and errors. Each
// frame is `{"seq","ts_ms","session_id","run_id","type",...}`.
//
// The stream is live only: there is no replay cursor. `seq` is monotonic within
// one server process, so a jump means frames were dropped - refetch the session
// (`GET /api/sessions/:id`), whose messages, runs and agents are persisted in
// SQLite. The pump says so explicitly with `stream_lagged` (`skipped` = frames
// lost, and a `seq` that is the last dropped one, so it never shadows a live
// event); that marker is transport, not session, and is exempt from `seq` dedupe.
// On a `?session=` stream, holes are normal because other sessions' frames are
// stamped and then filtered. `: keepalive` comments go out every 15 s and count as
// traffic: a client that receives no byte for about 45 s should treat the
// connection as dead and reconnect, because a half-open socket never reports an
// error by itself. A client that cannot drain its buffer for 5 s is disconnected
// rather than slowing the agent loop. A refetch must not replace a transcript that
// is still streaming - the snapshot only has persisted messages.
//
// # Errors
//
// Non-2xx answers use `{"error": {"code": "...", "message": "..."}}` with codes
// like `bad_request`, `not_found`, `permission_denied`, `payload_too_large`,
// `path_escape`, `conflict`, `unauthorized`. Messages name the field or the
// missing thing; secrets are never echoed.
//
// # Gotchas
//
// * `PATCH /api/config` validates the whole patch before writing: an invalid
//   field means nothing changes.
// * Unknown `/api/...` paths answer in JSON, never the SPA fallback.
// * A request with an unlisted `Host` is refused with `403` when the server is on
//   loopback without a token (DNS-rebinding defence); set
//   `KARUCODE_ALLOWED_HOSTS` for a proxy.
// * Long-running work is asynchronous: `POST .../messages` returns immediately and
//   progress comes over `/api/events`.
// * A session model pin is not a settings change: `PATCH /api/config` cannot set
//   it, and rotating the default in settings leaves pinned sessions where they
//   are until you clear the pin.
//
// Full reference: `docs/API.md` (request and response shapes for every route).

pub mod app;
pub mod bootstrap;
pub mod catalog;
pub mod error;
pub mod host;
pub mod metrics;
pub mod plugin_notifications;
pub mod plugins_watch;
pub mod routes;
pub mod scaffold;
pub mod settings;
pub mod state;
pub mod static_ui;
pub mod terminals;
pub mod titles;

pub use app::build_router;
pub use bootstrap::{Cli, CliAction};
pub use catalog::CatalogCache;
pub use host::HostAllowlist;
pub use settings::{ConfigPatch, SettingsHandle};
pub use state::{AppState, AppStateDeps};

/// Crate version, reported by `GET /api/health` and `--version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Parse the process arguments and either print help/version or run the server.
///
/// `main.rs` is intentionally a two-liner; keeping this here makes the CLI
/// parseable in unit tests and leaves room for a future library embedding.
pub async fn run() -> anyhow::Result<()> {
    match Cli::parse(std::env::args_os().skip(1))? {
        CliAction::Help => {
            println!("{}", Cli::usage());
            Ok(())
        }
        CliAction::Version => {
            println!("karucode {VERSION}");
            Ok(())
        }
        CliAction::PluginInit(args) => scaffold::init(&args),
        CliAction::PluginSdk(args) => scaffold::sync(&args),
        CliAction::Run(cli) => bootstrap::run(cli).await,
    }
}
