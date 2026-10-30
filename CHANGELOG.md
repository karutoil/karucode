# Changelog

All notable changes to karucode are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). While the major
version is `0`, a minor bump may contain breaking changes; they are called out
under **Changed**.

## [Unreleased]

### Security

- `GET /api/git/diff` no longer executes repository-controlled commands. The
  route was missing `--no-textconv`, so a cloned repository could run a
  `diff.<driver>.textconv` command through its own `.gitattributes` on a plain
  `GET` — no approval, no CSRF token, as the server user.
- `git diff` no longer executes `filter.<driver>.clean`. `--no-ext-diff` and
  `--no-textconv` do not cover clean filters, which `git diff` runs to compute
  the working-tree side. Drivers named in `.gitattributes` are now neutralised
  explicitly.
- Child processes no longer inherit the server's secrets. `run_command`,
  plugins and stdio MCP servers all received `KARUCODE_AUTH_TOKEN` and the
  provider API key through the environment; they now get an explicit allowlist.
- Project creation is restricted to operator-configured roots. Previously any
  API client could `POST /api/projects {"path":"/"}` and make the whole
  filesystem the confinement root for every file tool.
- Plugin tools can no longer exempt themselves from approval by declaring
  themselves `read_only`; the declared class is a hint, and the operator
  decides.
- Plugin permissions are deny-by-default. An absent `permissions` key used to
  mean "whatever the manifest asks for".
- `ask_commands` no longer auto-approves every file write. The behaviour now
  matches the documented table.
- Changing the approval mode or the model endpoint requires the admin
  credential, so an ordinary API client can no longer remove the human gate and
  then repoint the provider at itself.

### Fixed

- DeepSeek tool calls written as text now run. DeepSeek-V4 sometimes emits its
  native DSML markup (`<||DSML||tool_calls><||DSML||invoke name="...">…`) inside
  the ordinary `content` channel, and an OpenAI-compatible endpoint that does not
  parse it forwards the markup as prose — so the call never ran, the model
  printed XML at the user, and it looked like the tools were broken. The provider
  now recovers those blocks into real tool calls, including markers split across
  network chunks and the degraded `||DSML||` spelling, and keeps
  `string="true"`/JSON parameter typing. A block naming a tool that was not
  offered in that request is left as text rather than executed, so a model
  explaining the format cannot fire a call.
- The project rail's favicons were fetched successfully and then blocked by the
  page CSP: the icons are rendered from `blob:` object URLs, but `img-src` was
  `'self' data:` only. `blob:` is now allowed (it inherits the page's own origin,
  so it cannot load or leak anything from another host) while remote images stay
  blocked.
- `GET /api/projects/:id/favicon` answers `204 No Content` instead of `404` when
  a project ships no favicon. A missing favicon is the common case, and browsers
  log every 4xx as `Failed to load resource`, so a healthy deployment looked
  broken in the console. An unknown project id is still a `404`.
- A truncated provider stream is no longer persisted as a clean, successful
  answer. The provider synthesised `finish_reason: "eof"` and the runtime had no
  handling for it, so a proxy cutting the body mid-answer produced a
  `completed` run with silently truncated text.
- A tool call can no longer wedge a session forever. `tool.invoke` is now
  bounded by the run deadline and by cancellation, so one hung plugin or MCP
  tool no longer blocks every later run on that session with a 409.
- `search_files` no longer returns zero matches when `path` is a single file.
  ripgrep omits the filename prefix for a single file argument, and the parser
  required one, so every match was silently dropped and the tool reported
  "no matches" with `ok: true`.
- `search_files` enforces a global result cap and streams ripgrep's output
  instead of buffering all of it. `--max-count` is per file, not global.
- `search_files` shows long matching lines instead of the literal text
  `[Omitted long matching line]`.
- `read_file` reports the real total line count for files larger than the byte
  cap, instead of counting lines in the truncated buffer.
- `run_command` kills the whole process group on timeout even when the group
  leader exits on `SIGTERM` while a grandchild ignores it.
- Startup recovery is per-session best-effort: one corrupt session no longer
  prevents the server from starting, and recovery no longer reads every
  session's full transcript before the port is bound.
- Path claims held by interrupted subagents are released on recovery instead of
  warning about overlap forever.
- `ToolCallAccumulator` rejects an out-of-range tool-call index instead of
  allocating on an attacker-controlled value.
- Context compaction no longer orphans a tool result at the protected-tail
  boundary, which produced a request the provider rejects with a 400.
- `GET /api/health` returns the documented `200 degraded` instead of a 500 when
  the store is failing.
- Oversized request bodies return the documented JSON error shape instead of a
  plain-text 413.
- The web client no longer loses tool cards, approvals and notices when the
  event stream drops mid-answer, and it surfaces truncated history instead of
  silently showing a transcript whose beginning is missing.
- The web client has an error boundary, request timeouts, and scopes composer
  drafts to their session.

### Added

- Transcript view options, saved with the theme. A long session used to be a wall
  of equally-bulky tool cards with the reasoning collapsed to a 10px chip and the
  agent's between-calls prose styled exactly like its answer. The Theme Studio
  gained a **Transcript** section with three settings, each also carried by an
  exported theme JSON: **Tool calls** (`cards` — the existing card — or `rows`, a
  one-line call that expands into the same numbered output), **Grouping** (`none`,
  `tools` to fold a run of consecutive calls into one summary line, or `turns` to
  also fold a finished turn's working steps behind a "Show the working · N tool
  calls" line) and **Reasoning** (`collapsed`, `preview` for the first line of
  thought, or `expanded`). Prose written between tool calls is now drawn as
  narration — quieter and smaller than the answer — so the conclusion of a long
  turn stands out. The defaults (`cards`/`none`/`collapsed`) reproduce the
  previous look exactly, and themes saved before the options existed still load.
  Nothing that can still change what you must do is ever folded: a streaming
  turn, a running call, a pending approval, an error, an interrupted run and a
  notice stay visible, a group opens itself while a member is running or after a
  failure, and a notice or error that fell before the answer is lifted out of the
  fold rather than hidden.
- Hot plugin reloads, so installing, upgrading or removing a plugin no longer
  needs a server restart. `POST /api/plugins/reload` re-reads the `[[plugins]]`
  entries and every `<data_dir>/plugins/*/plugin.toml`, then starts new plugins,
  drops removed ones and replaces changed ones, leaving everything unchanged
  running untouched (it answers with a `{added, removed, restarted, unchanged}`
  report). `POST /api/plugins/:name/reload` replaces one plugin's process. A
  rebuilt binary counts as a change: each command argument that resolves to a
  file is fingerprinted by mtime and size, so "recompile, then reload" works even
  though the command line is byte-identical. Set `plugins_watch = true`
  (`KARUCODE_PLUGINS_WATCH`, interval `plugins_watch_secs` /
  `KARUCODE_PLUGINS_WATCH_SECS`, clamped to 1..=600) to have the server apply
  those changes on a timer instead of on demand. The tool registry is rebuilt
  before each reply, and a runtime enable/disable survives a rescan. The settings
  sheet grew Reload/Enable/Disable per plugin and a Rescan button.
- The project rail draws each project's own `favicon.ico` instead of its
  initials when the project ships one, found by `GET /api/projects/:id/favicon`
  (`.gitignore`-aware, shallowest match wins, `404` when there is none). The
  icon is fetched through the API client, so it also works on a deployment with
  `KARUCODE_AUTH_TOKEN` set, where an `<img src>` could not send the bearer
  token.
- CI pipeline: formatting, clippy, tests on Linux and macOS, the release
  profile, the docs gate, the web build, the end-to-end smoke suite, a
  dependency audit and an MSRV check, plus tag-triggered release artifacts.
- `deny.toml` dependency policy and a Dependabot configuration.
- `LICENSE` (MIT) and this changelog.
- Spend accounting: token and cost columns on runs, a per-session rollup, and
  totals in `GET /api/stats`.
- Request logging with correlation ids, and a metrics endpoint.
- SSE `id:` frames and `Last-Event-ID` resume, so a reconnect replays the gap
  instead of forcing a refetch that discards unsaved output.
- Data retention and vacuum, a single-instance lock, and database integrity
  checking.
- `Dockerfile`, a systemd unit and an install script.

### Changed

- `[profile.release]` uses `panic = "unwind"`. `panic = "abort"` silently
  disabled the runtime's `catch_unwind` guards in release builds only, so a
  panicking tool aborted the whole server while the tests proved it was
  contained.
- Provider retries are classified. Permanent 4xx responses are no longer
  retried 51 times with backoff, which turned a bad model id into a twenty
  minute hang.
- `max_output_tokens` is sent on the wire, so the context budget's output
  reserve matches what the model is actually allowed to generate.
- `KARUCODE_PROJECT_ROOTS` is an opt-in allowlist, not a default restriction.
  With the variable unset, `POST /api/projects` accepts any existing directory
  (except `/`) — previously it silently fell back to "the default workspace
  only", so the normal second project, a sibling checkout such as
  `~/projects/other-repo`, was refused with "outside the allowed project roots"
  until the operator set an environment variable. Set the variable (or
  `project_roots`) to restrict project creation on a shared or remotely
  reachable deployment. `/` is still refused, so no single request can make the
  whole filesystem the confinement root for every file tool.
- The composer **floats over the transcript** instead of being an attached band
  at the bottom of the pane, so the conversation keeps the full height of the
  centre column. It is one line tall at rest and grows with the draft. While you
  are scrolled into history it turns see-through (42% of the input surface with
  a 3px backdrop blur) and its status footnote slides away, so the messages
  behind it stay readable; clicking into the box restores the solid surface and
  the footnote. The transcript reserves the dock's measured height, so the last
  message never hides under it, and **Jump to latest** now sits above the box
  rather than under it. The box's own arrow keys, `@`-mention popover and dropup
  menus are unchanged.

## [0.1.0] - 2026-10-28

Initial release: multi-step agent loop, streaming, approvals, projects,
subagents, plugins, MCP, and the browser workspace.
