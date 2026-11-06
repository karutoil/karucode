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
// @kc-applies-to: route:GET /api/health, route:GET /api/config, route:PATCH /api/config, route:POST /api/provider/test, route:GET /api/models/catalog, route:GET /api/models/lookup, route:GET /api/stats, route:GET /api/projects, route:POST /api/projects, route:PATCH /api/projects/:id/preferences, route:GET /api/projects/:id/favicon, route:GET /api/sessions, route:POST /api/sessions, route:GET /api/sessions/:id, route:GET /api/sessions/:id/info, route:PATCH /api/sessions/:id, route:PATCH /api/sessions/:id/model, route:POST /api/sessions/:id/messages, route:GET /api/sessions/:id/queues, route:DELETE /api/sessions/:id/queues/:queued_id, route:POST /api/sessions/:id/cancel, route:GET /api/sessions/:id/rewind, route:POST /api/sessions/:id/rewind, route:GET /api/control/sessions, route:POST /api/control/sessions, route:GET /api/control/overview, route:GET /api/agents, route:GET /api/agents/:id, route:POST /api/agents/:id/pause, route:POST /api/agents/:id/resume, route:POST /api/agents/:id/cancel, route:GET /api/approvals, route:POST /api/approvals/:id, route:GET /api/workspace/tree, route:GET /api/workspace/file, route:PUT /api/workspace/file, route:GET /api/workspace/raw, route:GET /api/workspace/preview/*rest, route:POST /api/workspace/upload, route:POST /api/artifacts, route:GET /api/artifacts/:id, route:GET /api/git/status, route:GET /api/git/diff, route:GET /api/git/branches, route:POST /api/git/branches, route:POST /api/git/branches/delete, route:POST /api/git/checkout, route:GET /api/git/log, route:GET /api/git/commits/:sha, route:POST /api/git/stage, route:POST /api/git/unstage, route:POST /api/git/discard, route:POST /api/git/commit, route:GET /api/git/stash, route:POST /api/git/stash, route:GET /api/plugins, route:POST /api/plugins/reload, route:POST /api/plugins/:name/reload, route:POST /api/plugins/:name/enable, route:GET /api/plugins/:name/panels/:panel, route:GET /api/mcp, config:plugin_panels, route:GET /api/events, event:notification, event:queued_messages, event:session_rewound, config:[notifications], header:X-Karucode-Client, env:KARUCODE_AUTH_TOKEN, doc:docs/API.md
//
// The API is same-origin only; there is no CORS surface. Two rules matter:
//
// 1. **Auth** - with `KARUCODE_AUTH_TOKEN` set, every request needs
//    `Authorization: Bearer <token>`. Two paths also accept master `?token=`,
//    because a browser cannot attach a header to what they load: `GET /api/events`
//    (a stream fetch) and `GET /api/terminals/:id/socket` (a websocket handshake).
//    Artifact iframes instead use the random per-artifact `?capability=` returned
//    by their publishing POST; the master bearer is never put in those URLs.
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
// | `GET`/`POST /api/projects` | list and create projects; `PATCH /api/projects/:id/preferences` stores the Workbench result width; `GET /api/projects/:id/favicon` serves the project's own `favicon.ico` (`204` when it has none) |
// | `GET`/`POST /api/sessions` | list, create, load sessions |
// | `GET /api/sessions/:id` | one session with its transcript (`?messages=N`, default 5000, max 50000) |
// | `GET /api/sessions/:id/info` | the session's counts and token totals: messages, turns, tool calls, working time, per-agent usage, window fill |
// | `PATCH /api/sessions/:id`, `PATCH /api/sessions/:id/model` | rename, and pin or clear this session's model |
// | `POST /api/sessions/:id/messages` | start a run (SSE progress arrives on `/api/events`); `?mode=steer` folds the message into the run in flight, `?mode=follow_up` starts one after it |
// | `GET /api/sessions/:id/queues`, `DELETE /api/sessions/:id/queues/:queued_id` | the messages waiting for the run, and recall one (it never reaches the model) |
// | `POST /api/sessions/:id/cancel` | cancel the active run |
// | `GET`/`POST /api/sessions/:id/rewind` | preview, then hide the transcript after a message (optionally reverting its file changes, retrying it, or replacing it with a note) |
// | `GET /api/agents`, `GET /api/agents/:id` | every subagent in a project, and one agent's own transcript |
// | `POST /api/agents/:id/pause`, `/resume`, `/cancel` | pause a live subagent between iterations, release it, or stop it |
// | `GET /api/approvals`, `POST /api/approvals/:id` | list and answer pending approvals |
// | `GET /api/workspace/tree`, `/api/workspace/file` | workspace explorer, confined to the project |
// | `PUT /api/workspace/file`, `GET /api/workspace/raw` | save a file from the editor, and serve its raw bytes for image/PDF previews |
// | `POST /api/workspace/upload` | attach a file for the composer |
// | `POST /api/artifacts`, `GET /api/artifacts/:id` | publish a chat artifact (an HTML mock from the transcript) and serve it as an isolated page |
// | `GET /api/git/status`, `/api/git/diff` | working-tree state and one file's patch |
// | `GET /api/git/branches`, `POST /api/git/checkout` | local branches, and switching to one |
// | `POST /api/git/branches`, `POST /api/git/branches/delete` | create (and optionally switch to) a branch, delete one |
// | `GET /api/git/log`, `GET /api/git/commits/:sha` | commit history (optionally for one path), and one commit's files plus patch |
// | `POST /api/git/stage`, `/unstage`, `/discard` | the three per-file verbs, singly or by path list or whole workspace |
// | `POST /api/git/commit` | commit (or amend), optionally staging everything first |
// | `GET`/`POST /api/git/stash` | the stash stack, and push/pop/apply/drop |
// | `GET /api/plugins`, `POST /api/plugins/:name/enable` | plugin status, enable/disable |
// | `POST /api/plugins/reload` | rescan every plugin source: start new plugins, drop removed ones, replace changed ones (no server restart) |
// | `POST /api/plugins/:name/reload` | replace one plugin's process (after a rebuild or a manifest edit) |
// | `GET /api/plugins/:name/panels/:panel` | one plugin panel's payload for the inspector |
// | `GET /api/mcp` | MCP server status and discovered tools |
// | `GET /api/events` | the SSE event stream for the open session |
//
// # Chat artifacts
//
// A reply can carry a fenced block the web UI draws as live content - an HTML
// mock, a diagram, a table. That is a *rendering* of the reply, so the two routes
// exist only to give an `<iframe>` something to load:
//
// ```bash
// curl -s -X POST localhost:8787/api/artifacts \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"html":"<button>Buy</button>","theme":"dark"}'
// # → 201 {"id":"9f2c…","url":"/api/artifacts/9f2c…?capability=<random>","bytes":374,"theme":"dark"}
// ```
//
// The body is `{html, theme?}` (1 MiB; `413` above it, `400` when empty), the id
// is a SHA-256 of the bytes actually served, and the store is **in memory only**
// with a bounded size: the same artifact posted twice is one entry and one id, an
// evicted or forgotten one is a `404` the client answers by posting again, and a
// restart simply empties it. A fragment is wrapped in a small document whose
// background follows `theme`; a complete document keeps its own CSS and gets one
// injected script that reports its height to the parent, so the card is not a
// fixed letterbox.
//
// `GET /api/artifacts/:id?capability=<random>` serves that document only when the
// capability matches this artifact. The authenticated POST response's `url` is the
// value to load; an id alone or a master `?token=` is `401`, and reposting the same
// stored document returns its existing grant. The GET has a route-local policy that
// is stricter than the workspace preview's: an **opaque origin** (`sandbox
// allow-scripts`, never `allow-same-origin`), inline script and style allowed so a
// mock is a mock, while fetch/XHR/WebSocket, remote subresources and forms are
// disabled (`connect-src 'none'`, `form-action 'none'`, no wildcard source).
// The frame's sandbox does not prevent a directly opened top-level artifact from
// navigating itself; Chromium confirms that residual can disclose the viewing event
// and this one URL capability, but never the master bearer. `no-referrer` keeps the
// capability out of Referer headers. `frame-ancestors 'self'` plus route-local
// `Cross-Origin-Resource-Policy: cross-origin` let only this app frame it.
//
// How an agent writes one, and what does and does not render, is the `artifacts`
// topic.
//
// # Probing a provider
//
// `POST /api/provider/test` is a live probe: it sends the endpoint a `GET
// /models` (falling back to a one-token chat when the gateway does not implement
// it) and answers `200` with `{ok, models, error, latency_ms}` - a failing
// endpoint is a result, not an HTTP error. Every field of the body is optional:
//
// ```bash
// # the configured endpoint, with the configured key
// curl -s -X POST localhost:8787/api/provider/test \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' -d '{}'
// # a saved profile (and optionally one of its models)
// ... -d '{"profile":"gateway","model":"qwen3-coder"}'
// # a draft endpoint the user is still typing - it needs its own key
// ... -d '{"base_url":"http://127.0.0.1:4000/v1","api_key":"sk-…","model":"m"}'
// ```
//
// **A `base_url` override never carries the stored key.** Naming another endpoint
// without an `api_key` drops the configured credential, so the probe goes out
// keyless (right for a local gateway, a clean failure anywhere else) instead of
// replaying the operator's secret at a host the caller chose. This route is not
// part of the admin gate and ignores `lock_settings`, so that rule is the whole
// defence: supply both halves, or neither.
//
// # Reading a long transcript
//
// A session with thousands of tool calls has far more messages than one screen.
// `GET /api/sessions/:id` returns the **newest** `messages` rows (default 5000,
// `?messages=N` up to 50000) and three fields that make the cut explicit instead
// of silent:
//
// ```json
// { "messages": [ ... ], "messages_limit": 5000, "messages_truncated": true,
//   "messages_before": 4211 }
// ```
//
// The page is bounded by **stored payload bytes** (about 1 MiB), not by the row
// count alone, so a handful of large tool results fills it before `?messages=`
// does; one oversized message is returned alone so paging always progresses.
// That keeps both the response and the server's transient copies proportional to
// one page rather than to the whole transcript.
//
// `messages_truncated: true` means older visible rows remain; `messages_before`
// is the exclusive row-ID cursor, and `?before=<id>` asks for the page older than
// it. `GET /api/agents/:id` behaves the same way for one agent's transcript.
// Nothing is deleted. A non-numeric `messages` or `before` is `400 bad_request`.
//
// An assistant row may also carry `reasoning_content` when the endpoint streams
// analysis separately from the answer. That text is stored with the turn and
// sent back in later provider requests; it is omitted for older rows and for
// models that do not produce a separate reasoning stream.
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
// # Project preferences
//
// `PATCH /api/projects/:id/preferences` with
// `{"result_panel_width":580}` stores the Workbench result width for that project;
// `GET /api/projects` returns it to every fresh browser or device. It accepts an
// integer from 280 through 1600 pixels and never changes model/run configuration,
// sessions or terminals. The browser can temporarily clamp a width to its viewport
// without persisting that clamp. See `docs/API.md` for malformed-input responses.
//
// ```bash
// curl -X PATCH localhost:8787/api/projects/p_ab12/preferences \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"result_panel_width":580}'
// ```
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
// endpoint cannot be an `<img src>` or an embedded PDF. The response carries its
// own `Content-Security-Policy: sandbox; default-src 'none'`, which makes the
// document an opaque origin with script disabled - so a workspace HTML file served
// here cannot execute, even though it shares the app's origin and its content type
// is guessed from the file name. That sandbox CSP is why this route is safe; the
// app-wide CSP alone would not be, because its `script-src` falls back to `'self'`
// and the response *is* `'self'`. The file preview depends on the response staying
// `inline` and frameable (an `<img src>` and a PDF `<iframe>` both hotlink it), so
// do not "harden" it into an attachment.
//
// Note that `binary` from the read endpoint is a NUL-byte sniff, and a PDF or an
// SVG is ASCII - so it is `false` for both. Decide how to display a file from its
// extension, not from that flag.
//
// # Previewing an HTML file as a page
//
// `GET /api/workspace/preview/<path>[?project=<id>]` serves a workspace file
// from its own workspace path, so the web viewer can show an HTML page the way
// its author meant it: the page's `./style.css`, `../img/logo.png` and
// `./app.js` resolve against the directory the page lives in, and its inline
// scripts run.
//
// ```bash
// curl -s -D- -o /dev/null "localhost:8787/api/workspace/preview/docs/index.html"
// # content-type: text/html; charset=utf-8
// # content-security-policy: sandbox allow-scripts; default-src 'none'; …
// ```
//
// The path is in the URL rather than a query parameter for exactly that reason:
// a document has to have a base URL inside the workspace. (`srcdoc` cannot work
// for the same preview - it inherits the app's own `default-src 'self'` policy
// and then runs in an opaque origin where `'self'` matches nothing, so its
// inline scripts, sibling stylesheets and relative images are all refused.)
//
// The route serves the whole preview *tree*: the same prefix answers the page's
// assets, so a reviewed page looks like the page.
//
// What makes it safe, and what to preserve if you change it:
//
// * the document answers `Content-Security-Policy: sandbox allow-scripts; …`,
//   which is an **opaque origin**. It cannot read the app's DOM,
//   `localStorage` or cookies, cannot `fetch`/XHR/WebSocket (`connect-src
//   'none'`), cannot open a popup, cannot navigate the top frame and cannot
//   submit a form. Never add `allow-same-origin` to that policy or to the
//   viewer's `<iframe sandbox>`: that pair hands the page the app's origin,
//   where it could read the stored bearer token and call `/api`;
// * assets keep the stricter `sandbox; default-src 'none'` used by
//   `/api/workspace/raw`;
// * the response widens `Cross-Origin-Resource-Policy` to `cross-origin` so the
//   opaque-origin page may load its own siblings - the app-wide `same-origin`
//   would block every one of them. It is widened on this route only, and CORP
//   never grants *read* access: with no `Access-Control-Allow-Origin`, a foreign
//   page can at most render a workspace file, never read it;
// * a directory answers its `index.html` (`400` when there is none), a
//   non-UTF-8 document is `400`, and anything over 16 MiB is `413
//   payload_too_large`.
//
// A deployment that sets `KARUCODE_AUTH_TOKEN` cannot use this preview: an
// `<iframe>` is a browser navigation and cannot send `Authorization`, and the
// page's own relative requests cannot carry a query token either. The viewer
// says so instead of showing a blank frame; the *Source* tab still works.
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
//   status is `400 bad_request`). The response also carries `running` (alive and
//   spending, which includes `paused`), `paused` and `live` (the runtime's own
//   count of loops that exist right now), so a badge does not have to count the
//   array. Without `?project=` it lists every agent on the server.
// * `GET /api/agents/:id` returns `{ agent, messages, messages_truncated,
//   messages_limit }`: only that agent's messages, oldest first, tail-limited by
//   `?messages=N` (default 2000, max 50000). This is the drill-in transcript.
//
// ```bash
// curl -s 'localhost:8787/api/agents?project=p_ab12&status=running' | jq '.running, .agents[].name'
// curl -s 'localhost:8787/api/agents/a_x?messages=200' | jq '.agent.status, (.messages | length)'
// curl -s -X POST 'localhost:8787/api/agents/a_x/pause' | jq '.status'
// curl -s -X POST 'localhost:8787/api/agents/a_x/resume' | jq '.status'
// curl -s -X POST 'localhost:8787/api/agents/a_x/cancel' | jq '.status'
// ```
//
// The control routes answer `{ agent_id, action, status }`. A pause takes effect
// at the agent's next iteration: the turn in flight finishes first, because that
// is the only point where stopping costs no work, so the status is `paused`
// immediately and the agent keeps that status until it is resumed or stopped.
// `cancel` fires the agent's token at once; the response says `cancelled` while
// the transcript catches up a moment later. An unknown id, or one whose loop has
// already finished, is `404 not_found` - a browser that raced the finish is told
// rather than given a pause that will never land. An unknown verb is `400
// bad_request` naming the three.
//
// The live event stream carries the same activity: `agent_spawned`,
// `agent_progress`, `agent_paused`, `agent_message`, `agent_note` and
// `agent_finished` for lifecycle, and the normal `text_delta`,
// `tool_call_started/finished`, `command_*` and `approval_*` frames stamped with
// the subagent's `agent_id`. One SSE connection therefore feeds every agent in
// every session; no per-agent stream exists.
//
// # The control plane
//
// A control chat is an ordinary session that belongs to **no project**: its
// `project_id` is `null`, its workspace is a private directory
// (`<data_dir>/control-workspaces/<session_id>`) and it reaches projects only
// through its own tools (see the `central-control` topic). Any number of them can
// exist at once, so `POST` creates one rather than reopening a singleton.
//
// ```bash
// # Open a chat with its opening request. Without a `title` the chat is named
// # from that prompt: deterministically at once, then by a model when [titles]
// # is on (the default).
// curl -s -X POST localhost:8787/api/control/sessions \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"prompt":"audit the open pull requests"}'
// # 200 {"session_id":"s_ab12","project_id":null,"workspace":"/…/control-workspaces/s_ab12",
// #      "title":"audit the open pull requests","created":true}
//
// # Every chat, newest activity first: a SessionSummary each, plus
// # agents {live,total} and the model the chat would use right now.
// curl -s localhost:8787/api/control/sessions | jq '.sessions[] | {title, model_label, agents}'
// ```
//
// `GET /api/control/overview` is the whole plane in one read-only snapshot, for a
// dashboard that would otherwise make five requests: `sessions {total, running,
// failed, control}` (the first three count **project sessions only**; control
// chats are the fourth), `agents {live, paused, failed, total}`, `approvals
// {pending}`, `projects` (with `sessions` and `running` per project), `control`
// (the rows of the listing above), `running` / `attention` / `waiting` / `recent`
// (project sessions only, at most 12 each; `attention` means the latest run ended
// `failed`, `interrupted` or `limit_reached`, and `waiting` means an unanswered
// approval) and `live_agents` (running or paused, most recently active first,
// with the session each belongs to). Control chats never appear in the
// project-session lists; they are their own array.
//
// A chat is named by its **first message** if it was created with neither a title
// nor a prompt, so `POST /api/sessions/:id/messages` is where an untitled chat
// gets its name. A `prompt` passed to `POST /api/control/sessions` is only a
// naming hint - it is not stored as a message and starts no run. An explicitly
// titled chat is never renamed - see the `session-titles` topic for the model, the
// prompt and what is refused.
//
// ```bash
// curl -s localhost:8787/api/control/overview | jq '.sessions, .agents, (.control | length)'
// ```
//
// # Managing the working tree
//
// The git routes are the source-control manager's whole backend. They are
// **local only**: staging, commits, branches, stashes and history. There is no
// fetch/pull/push route on purpose - those need credentials and can block on a
// prompt, so they stay in the terminal, where the operator already is.
//
// Reads:
//
// ```json
// GET /api/git/status
// { "is_repo": true, "branch": "main", "detached": false,
//   "upstream": "origin/main", "ahead": 1, "behind": 0, "error": null,
//   "truncated": false,
//   "files": [ { "path": "src/x.rs", "orig_path": null,
//                "index": "M", "worktree": " ", "status": "M",
//                "staged": true, "unstaged": false,
//                "untracked": false, "conflict": false } ] }
// ```
// `index`/`worktree` are the two porcelain columns, so a path that is both
// staged and modified (`MM`) appears in both lists of the UI; `status` keeps the
// older trimmed form. `detached` means `branch` is `null`.
//
// Every mutation answers the same envelope, with a *fresh* status so a client
// never races a second `GET` against a command it just ran:
//
// ```json
// { "ok": true, "message": "staged 2 files", "status": { … }, "commit": null }
// ```
//
// ```bash
// curl -s -X POST localhost:8787/api/git/stage -H 'X-Karucode-Client: web' \
//   -H 'Content-Type: application/json' -d '{"paths":["src/x.rs"]}'
// curl -s -X POST localhost:8787/api/git/commit -H 'X-Karucode-Client: web' \
//   -H 'Content-Type: application/json' -d '{"message":"fix: x","stage_all":true}'
// curl -s -X POST localhost:8787/api/git/branches -H 'X-Karucode-Client: web' \
//   -H 'Content-Type: application/json' -d '{"name":"feat/y","checkout":true}'
// ```
//
// What each body takes:
//
// * `stage` / `unstage`: `{"paths":[…]}` or `{"all":true}`.
// * `discard`: the same, plus `"staged": true` to restore the index *and* the
//   working tree to `HEAD` instead of the working tree to the index, and
//   `"confirm": true` - **required**, because an untracked path is deleted from
//   disk and nothing here can undo it.
// * `commit`: `{"message":"…","amend":bool,"stage_all":bool}`. An empty message
//   that amends keeps the previous one (`--no-edit`).
// * `branches`: `{"name":"…","start":"…","checkout":bool}`;
//   `branches/delete`: `{"name":"…","force":bool,"confirm":bool}` (`force` needs
//   `confirm`); `checkout`: `{"name":"…"}`.
// * `stash`: `{"action":"push|pop|apply|drop","index":0,"message":"…",
//   "include_untracked":bool,"confirm":bool}`. `drop` needs `confirm`.
//
// A refused command is `400 bad_request` carrying git's own message (nothing to
// commit, a dirty tree that would be overwritten, an unmerged branch, an unknown
// ref), so the caller can show it verbatim. `GET /api/git/commits/:sha` answers
// `404` for a well-formed id that is not in this repository.
//
// Safety rules the routes keep, and that any new git route must keep too:
//
// * `git` is spawned with an argv array, never a shell, with the workspace root
//   as `current_dir`; an inherited `GIT_DIR`/`GIT_WORK_TREE` is dropped;
// * every path is resolved through the workspace first, so `..`, an absolute
//   path outside it and a symlink escape are refused before git runs; the
//   pathspec is passed after `--` with `GIT_LITERAL_PATHSPECS=1`, so a file
//   named `a[1].txt` (or a `*.rs` glob) can never match more than it names;
// * refs and branch names are validated against the shared `kc_tools::git`
//   grammar before they sit in a flag position, and a commit id must be hex;
// * repository config may not execute anything on a *read*: `core.fsmonitor`,
//   `diff.external`, the pager and attribute-selected diff drivers are pinned by
//   `kc_tools::git::SAFE_CONFIG`, and `diff` neutralises every
//   `filter.<driver>.*` the repository configures. Mutations do **not**
//   neutralise hooks or filters: `git add`/`commit` run `pre-commit` and LFS
//   exactly like a terminal command, which is what the person clicking expects;
// * nothing waits forever: reads are bounded at 10 s, mutations at 60 s (a hook
//   or a commit-signing program is the slow part), and each spawn is
//   `kill_on_drop`.
//
// Commits need a git identity. karucode never invents one: set `user.name` and
// `user.email` in the repository or in the service account's `~/.gitconfig`, or
// the commit is refused with git's "please tell me who you are" message.
//
// An untracked path still gets a readable patch: `GET /api/git/diff?path=…`
// compares it against the empty file, so "new file" reads as added lines instead
// of "no diff".
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
pub mod artifacts;
pub mod bootstrap;
pub mod catalog;
pub mod error;
pub mod host;
pub mod metrics;
mod mission_coordinator;
pub mod plugin_notifications;
pub mod plugins_watch;
pub mod retention;
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

pub mod control;
pub mod control_sessions;
pub mod memory;
