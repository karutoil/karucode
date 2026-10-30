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
// @kc-applies-to: route:GET /api/health, route:GET /api/config, route:PATCH /api/config, route:POST /api/provider/test, route:GET /api/models/catalog, route:GET /api/models/lookup, route:GET /api/stats, route:GET /api/projects, route:POST /api/projects, route:GET /api/projects/:id/favicon, route:GET /api/sessions, route:POST /api/sessions, route:GET /api/sessions/:id, route:PATCH /api/sessions/:id, route:PATCH /api/sessions/:id/model, route:POST /api/sessions/:id/messages, route:POST /api/sessions/:id/cancel, route:GET /api/agents, route:GET /api/agents/:id, route:GET /api/approvals, route:POST /api/approvals/:id, route:GET /api/workspace/tree, route:GET /api/workspace/file, route:PUT /api/workspace/file, route:GET /api/workspace/raw, route:POST /api/workspace/upload, route:GET /api/git/status, route:GET /api/git/diff, route:GET /api/plugins, route:POST /api/plugins/reload, route:POST /api/plugins/:name/reload, route:POST /api/plugins/:name/enable, route:GET /api/plugins/:name/panels/:panel, route:GET /api/mcp, config:plugin_panels, route:GET /api/events, event:notification, config:[notifications], header:X-Karucode-Client, env:KARUCODE_AUTH_TOKEN, doc:docs/API.md
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
// | `PATCH /api/sessions/:id`, `PATCH /api/sessions/:id/model` | rename, and pin or clear this session's model |
// | `POST /api/sessions/:id/messages` | start a run (SSE progress arrives on `/api/events`) |
// | `POST /api/sessions/:id/cancel` | cancel the active run |
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
