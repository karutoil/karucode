# Changelog

All notable changes to karucode are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). While the major
version is `0`, a minor bump may contain breaking changes; they are called out
under **Changed**.

## [Unreleased]

### Added

- **A tabbed settings sheet.** The settings screen was one long scroll of every
  section at once, which made any one setting hard to find. It is now five tabs —
  **General** (appearance: theme, density, colour mode and the transcript layout,
  with a door into the Theme studio), **Models** (provider profiles and their
  model libraries), **Agent** (approvals and subagents), **Notifications** and
  **Plugins & MCP** — split by who owns the setting: this browser, the endpoint,
  the agent's guardrails, what it announces, and what is loaded.
- Tuning knobs whose defaults are right for nearly every install moved under a
  collapsed **Advanced** disclosure in their tab: sampling and the request
  timeout in *Models*, the subagent budget, timeouts and every raw limit in
  *Agent*, and the notification delivery knobs (*minimum run length*, *include
  the start of the answer*) in *Notifications*. Fonts and corner rounding in
  *General* likewise.
- The notification settings are now three sections rather than one: server
  routing, *this browser*, and delivery. The first is what the server announces
  for every client, the second is stored on the device and never travels, and
  the third decides whether a notification is worth sending.
- Appearance is editable without opening the Theme studio. Theme, density and
  colour mode apply immediately from *General*, alongside the transcript's tool
  style, grouping and reasoning-block options.
- A save that fails validation takes you to the field: the sheet switches to the
  tab that owns it, opens the disclosure it lives behind, and focuses the input,
  while the footer says how many settings need attention and in which tabs. The
  Save button is shared across tabs, so the failing field was otherwise
  off-screen.
- **Notifications.** karucode now tells you when a run settles, parks on an
  approval, or fails. The server emits one `notification` event per moment, with a
  `kind` (`run_finished`, `needs_input`, `error`, `info`), the session's title and
  a stable id; every client renders it. `[notifications]` (and
  `KARUCODE_NOTIFICATIONS*`) turns it off four different ways — globally, per event,
  for plugins only, or per browser surface — and `enabled = false` stops the event at
  the source rather than asking clients to ignore it.
- Notifications reach a human four ways: an in-app toast that jumps to the session,
  an unread count on the session row and in the tab title, a sound cue, and a
  desktop notification where the browser allows one. The cues are synthesized with
  Web Audio (the page CSP has no `media-src`, so an audio file would be refused) and
  are independent of the notification permission: a browser that refused
  notifications still plays the cue.
- Plugins can raise notifications with a `notify` JSON-RPC frame, validated,
  truncated and rate-limited by the host (1/s, burst 5) so a plugin cannot flood a
  desktop, and can observe every notification with the new `on_notify` hook — which
  is how a plugin bridges karucode to ntfy or a desktop daemon. The SDK gains
  `notify()`, `NotifyFrame`, `PluginBuilder::on_notify` and
  `PluginBuilder::notifications`.
- A run cancelled by the user is deliberately silent, and a subagent is reported
  only when it fails.

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

- The transcript stopped auto-following as soon as output got fast. While pinned
  to the newest message it kept up with streamed text, but a burst of big tool
  cards could unpin it for good: a browser frame dispatches the scroll event from
  a follow *before* the next one can measure it, so when the following frame
  measured a fresh `scrollHeight` against the `scrollTop` the follow had just
  written, the difference was the size of the card that arrived in between — well
  over the 48px pin threshold — and the view read its own backlog as the reader
  scrolling away. It now records where following left the viewport and treats a
  gap no wider than the content that has arrived since then as its own to close,
  so any number of cards can land in a frame. Any movement of the scroll position
  is still the reader's, and is never overridden.

- A turn whose prose had finished but whose tool call was still running showed
  the **Completed** chip. The status chip and the new throughput badge would then
  contradict each other on the same row — "Completed" beside a live token counter.
  Both now use the same rule (streaming, a running call, or a pending approval
  means **Working**), which is also the rule that keeps a turn unfolded.
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

- `packaging/update.sh`: one command to move a systemd install onto a fresh build
  of your own checkout, for the case the installer does not cover. `install.sh`
  installs a binary but never builds one, so anyone following `main` instead of a
  release was doing the upgrade by hand: `cargo build`, `sudo install`, then
  remembering every flag the unit was installed with. `sudo sh packaging/update.sh`
  now fetches, builds, snapshots the SQLite database (`VACUUM INTO`, online, no
  `-wal` sidecar) and hands the result to `install.sh`, which still owns every
  systemd step, so there is no second copy of that logic to drift. It reads the
  *running* unit (`ExecStart=`, `User=`, `WorkingDirectory=`, the `KARUCODE_*`
  lines, and `MemoryDenyWriteExecute=`, which becomes `--allow-jit`) instead of
  using the fresh-host defaults compiled into `install.sh` — otherwise the first
  update would rewrite `10-install.conf` to point at `/srv/karucode/workspace`
  and the service would die with `status=200/CHDIR`; a workspace that is missing
  is now a hard error rather than a guess. The build runs as the checkout's owner
  even under `sudo`, so `target/` is not left root-owned. A dirty tree is refused
  (`--allow-dirty` overrides) so the shipped binary is the reviewed revision, and
  an unchanged binary plus unchanged unit skips the restart instead of dropping
  in-flight sessions (`--force` reinstalls). `--check` reports pending commits as
  exit `3` without changing anything, `--dry-run` prints the whole plan, and
  `--build-only`, `--skip-build`, `--no-pull` and `--ref` cover the offline and
  reproduce-a-tag cases. It refuses to run from inside the service, where
  `NoNewPrivileges=yes` makes `sudo` impossible.
- The update is **staged before it is switched**. Every failure that can be
  caught without touching the live service is now caught there: the built binary
  and the unit file are copied into `<data-dir>/stage`, and *those copies* are
  validated — the binary has to run (`--version`), the unit has to contain a
  `[Unit]` section, and a sha256 is recorded in a manifest that is written last.
  Only then does anything move the binary, rewrite `10-install.conf` or restart
  the unit, so a binary built for the wrong libc or a half-written file can no
  longer take a working service down. `--stage-only` stops after the stage and
  prints the version and checksum, which is the maintenance-window form: build
  and prove one day, switch the next with `--from-stage` (no rebuild, no fetch).
  `--from-stage` re-checks the manifest checksum and refuses a tampered or
  incomplete stage, and removes the stage after a successful switch so it cannot
  be applied twice. The stage is root-owned because it carries the artifacts
  across the privilege boundary: the build runs as the operator, `install.sh`
  reads as root.
- The update **survives losing the terminal**. A run that builds or switches now
  re-executes itself in its own session (`setsid`, its own log) before doing any
  work, and the shell that started it only waits and forwards the exit code, so
  closing the window or dropping an SSH connection can no longer abort an update
  part way through the switch — the one moment nobody can tell what state the
  install is in. `--follow` reattaches to the log afterwards, and
  `<data-dir>/update-job/` holds `log`, `pid` and the final `status` (exit code).
  It deliberately stays in the foreground when it cannot do so safely: with no
  terminal (a `sudo` password prompt would have nowhere to go), when `sudo -n`
  cannot escalate, and for `--check`/`--dry-run`/`--build-only`/`--stage-only`,
  where nothing live is at risk. `--detach`/`--no-detach` override the decision.
  Two updates can no longer run at once: a lock beside the data directory refuses
  the second run and names the pid holding it, distinguishes a stale lock from a
  live one, and reports a non-writable directory as such instead of blaming a
  phantom lock.
- `tests/test_update.py` runs `update.sh` as a real process against a real git
  repository with `systemctl`, `cargo` and `install.sh` stubbed: 72 cases, up from
  46. New coverage for staging (a binary that does not run, a file that is not a
  unit, a tampered stage, an incomplete stage, `--from-stage` not rebuilding,
  the stage being consumed), for detaching (a real SIGHUP to the waiting shell
  followed by a completed switch, and the no-tty case staying in the foreground),
  and for locking (a live lock, a stale one, one with no pid). Mutation-tested:
  dropping the staged-binary validation, making `--from-stage` rebuild, or
  forgetting to mark the detached child each fails a specific case. CI gained a
  `packaging` job running shellcheck and that suite.
- A live **throughput badge** and a **context-window pill**, the two readouts a
  long run needs. While an assistant turn is doing anything — streaming prose or
  reasoning, running a tool or command, or waiting on an approval — its header
  shows `↓ <tokens> <rate> t/s` beside the **Working** chip: the tokens produced
  so far and how fast they are arriving, both estimated from the streamed
  characters at the server's own four-chars-per-token ratio, and labelled as
  estimates in the tooltip because the provider's exact count only arrives when
  the call ends. The rate averages the last few seconds of *growth*, so a stream
  that has gone quiet (a 40 s `cargo test`) drops the number and keeps the count
  instead of freezing at the last burst. The badge is mounted only while the turn
  works, so it cannot outlive the run. The composer's bottom row gained
  `35.9%`-style **context usage**: the prompt size the provider reported for the
  newest call over the selected model's `context_window`, amber at 70% and red at
  90%, hidden entirely when either number is unknown rather than guessing. It
  reads the session's model pin (the same model the switcher beside it names) and
  remembers the last reported figure per session, so a reload does not blank it.
  On a phone the rate and the meter drop out and the number keeps its place, with
  the composer row still one line.
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
