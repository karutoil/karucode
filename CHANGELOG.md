# Changelog

All notable changes to karucode are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

karucode does **not** use semantic versioning. Every commit on `main` is a
release and the version is the git commit count, so build 107 is the 107th commit
and a release is tagged `v107`. `karucode --version` prints it. An entry that
matters to an operator says which behaviour changed, not which number.

## [Unreleased]

### Added

- **A version that is the commit count, and an update system to go with it.**
  `crates/kc-server/build.rs` stamps `git rev-list --count HEAD` (plus the commit,
  its date, a dirty marker, the channel and the target triple) into the binary, so
  `karucode --version` prints `karucode 107 (abf0a42, …)` and `GET /api/health`
  reports `version`/`build`/`commit`/`commit_date`/`dirty`; `/metrics` carries the
  same labels. There is no semver to bump and no release train - every commit on
  `main` is published as `v<build>`, with a tarball, the raw binary, `SHA256SUMS`
  and an `updates.json` manifest written by `scripts/release_manifest.py`.
  A released install updates itself: `karucode update --check` (exit 3 when a
  newer build exists), `--stage` (download, verify the SHA-256 and size, then run
  the artifact once with `--version` to prove it is the promised build) and
  `--apply` (atomic rename, previous binary kept as `karucode.old`, `--restart`
  restarts the systemd unit). The server checks on startup and every six hours
  (`[updates] check`, `check_secs`, `manifest_url`, `auto_apply`; env
  `KARUCODE_UPDATE_*`), raises an informational notification when a newer build
  exists, and serves it all at `GET /api/update`, `POST /api/update/check` and
  `POST /api/update/apply`. **Settings -> Updates** shows the running build, the
  release notes and an install button - or, because the service account usually
  cannot write `/usr/local/bin/karucode`, the exact `sudo` command that can. A
  source checkout is still updated with `packaging/update.sh` and a container by
  rebuilding its image; the server refuses both rather than pretending.
- **The release job now publishes on every push to `main`.** It builds with a
  full history (a shallow clone would stamp every build as build 1), asserts that
  `--version` matches the commit count, and uploads the tarball, the raw binary,
  `updates.json`, `SHA256SUMS`, `install.sh` and the unit. A pushed `v<build>` tag
  is accepted only when it names that commit's build. A new CI job proves the
  version stamp from a full-history checkout, and the manifest writer has its own
  test suite.

- **A first-run setup wizard, and a real sign-in screen instead of a pasted
  token.** A fresh install now opens six steps - welcome, model provider (base
  URL, key, model, with a live **Test connection**), appearance (theme, density,
  tool-call style as rows or cards, reasoning visibility), the default approval
  mode, access (password or passkey, public address, tunnel and proxy list) and a
  summary - each writing through the API that already owns that setting. It runs
  once: **Skip** records the same completion marker as finishing
  (`GET /api/onboarding`, `POST /api/onboarding/complete`).
- **Web-UI sign-in with a password or a passkey.** `[access] enabled = true` puts
  a sign-in screen in front of the app and exchanges a credential for an
  `HttpOnly` session cookie, so the browser no longer needs a bearer token pasted
  into `localStorage`. The password is Argon2id (only the hash is stored, in the
  SQLite settings table - never in `config.toml` or `GET /api/config`); a session
  cookie is 32 random bytes of which only the SHA-256 is persisted; eight wrong
  attempts from one address in 15 minutes lock that address out for 5. Passkeys
  are WebAuthn **ES256** with the full ceremony checked (single-use challenge,
  origin, rp id hash, user-present, non-decreasing counter) and attestation
  deliberately not. Creating the first credential turns sign-in on and signs that
  browser in, so setting a password cannot lock you out; removing the last
  credential turns it off; `[access] enabled` with no credential is a `400`.
  Settings → **Access** manages all of it: the switch, the password
  (set/change/remove), the passkey list, the signed-in browsers (sign one out, or
  everywhere), and the address/tunnel policy.
- **`[access]`: the address this server answers to, and which proxy may speak for
  a client.** `public_url` is added to the `Host` allowlist (so a tunnel's name is
  served without `KARUCODE_ALLOWED_HOSTS`), decides `Secure` on the cookie and is
  the origin a passkey is bound to; `tunnel = "cloudflare" | "custom" | "none"`
  and `trusted_proxies` decide whose `X-Forwarded-For` / `-Proto` / `-Host` are
  believed - **only the direct peer is checked**, loopback is always trusted, and
  `cloudflare` adds Cloudflare's published ranges automatically. `session_days`
  (30, 1..=365) bounds a sign-in. All of it is hot-swappable through
  `PATCH /api/config`. A non-loopback bind is now allowed on a password/passkey
  alone, provided the credential really exists.

- **A switch for parallel tool calls: `limits.parallel_tool_calls` (on by
  default).** A turn used to run every call one at a time except a batch of
  read-only ones, which ran concurrently as long as no mutating call was anywhere
  in the turn. Now a turn runs the calls that cannot see each other's effects at
  the same time: the read-only, approval-free ones, and **built-in file writes and
  edits whose paths are disjoint** when the approval policy would not prompt for
  them (`ask_commands`, `auto`, or an `auto_approve_paths` prefix in `ask`). Two
  writes to one path, or a read of a path the turn also writes, are not
  independent - the whole turn goes back to strict call order, because `write_file
  a` then `read_file a` has an order. `run_command`, plugin and MCP tools, subagent
  spawns, the `note` tool and anything that would park a worker on a human
  approval are never batched. Whatever overlaps, results are persisted and folded
  in **call order**, so the transcript and the next request are byte-identical to
  the sequential path. `parallel_tool_calls = false` (or
  `KARUCODE_PARALLEL_TOOL_CALLS=0`) is the strict-order escape hatch and runs every
  call on its own. It is on the settings screen ("Run independent tool calls in
  parallel"), in `GET`/`PATCH /api/config`, and distinct from
  `model.parallel_tool_calls`, which only asks the endpoint to let the model emit
  several calls per turn. Five runtime tests pin it: the switch off, disjoint
  writes overlapping, one path staying ordered (including the `./same.txt`
  spelling), a read/write pair staying ordered, and a write that would prompt
  keeping the turn sequential.

- **"New session" no longer creates an empty session.** The rail button,
  `Ctrl/Cmd+N`, the palette row, a starter prompt and the collapsed rail's fly-out
  all open a **draft** instead: an empty conversation the client holds, with the
  composer usable and the model / reasoning pills mounted, and *nothing* written to
  the server. The first message is what creates the session
  (`POST /api/sessions`, carrying the model pin chosen in the composer, so the
  first turn already runs on the model the pills showed) and is sent into it — the
  server titles the untitled session from that message, so the rail never collects
  rows nobody ever typed into. Opening another session or switching project
  discards the draft; a second press keeps the one you have rather than throwing
  away what is already typed; a failed create is reported and the draft, text and
  attachment chips included, stays put so sending again is the whole retry. The
  one exception is a file large enough to be *uploaded*: it lives under
  `.karucode/attachments/<session>/`, so attaching one creates the session it
  needs. `web/src/state/draftSession.test.ts` pins "the button writes nothing" and
  "the first message writes exactly one session".
- **Files open as tabs beside the conversation, not only as a modal.** A file still
  opens in the viewer dialog, and both the explorer's row menu (**Open in a tab**)
  and the viewer's own **Open in tab** button now put the same document in the
  workspace strip next to Conversation, Source control and Terminal; **Open in
  dialog** puts it back. Each tab keeps its read, its scroll position and an
  unsaved draft while you visit another tab, closing it selects the tab that took
  its place, and a tab with unsaved edits asks before it goes (the strip's ✕ goes
  through the same confirmation as the dialog's close). **Delete**/**Backspace**
  closes the selected tab. File tabs belong to the project, so switching projects
  closes them, and a link inside a rendered preview opens its own tab rather than
  replacing the page being read.
- **GitHub's Markdown, and images that actually load.** Tables, strikethrough,
  fenced code and bare-URL autolinks were already there; task lists (`- [ ]`,
  rendered as read-only checkboxes) and footnotes with in-page references now are
  too, added as markdown-it plugins with the sanitiser widened for exactly one
  element (a disabled checkbox) and a footnote reference no longer opening in a new
  tab. An image in a reply that points into the workspace is fetched with the
  session's credentials and shown from a `blob:` URL, shared per file so a
  scroll-windowed transcript does not re-download it on every pass; an `https:`
  image becomes a link, because the app's policy allows no remote image and a broken
  glyph only reads as a broken app; a reference that escapes the workspace says so.
- **HTML previews render as the page its author wrote.** The preview loaded its
  document and then nothing else: inside a `sandbox`ed (opaque-origin) document the
  CSP keyword `'self'` matches nothing, so every relative stylesheet, script and
  image was refused, and `?project=<id>` scoped only the entry document because a
  relative reference drops the query. The policy now names the request's own origin
  (`http://host:port` and its https twin) and adds no remote source - the page is
  still an opaque origin with `connect-src 'none'` that cannot reach the app, its
  storage or the network - and a file in a named project is served from the new
  path-scoped `GET /api/workspace/preview-project/<project>/<path>` so its assets
  stay in the same project. Verified in Chromium: the page's own script runs, its
  stylesheet applies and its relative image decodes; fonts and ES modules still
  cannot load, because both need CORS and this route deliberately sends no
  `Access-Control-Allow-Origin`.

- **Panel views: a plugin's data drawn as a real view, without shipping UI.** A
  panel payload used to land on the generic JSON tree, which is honest and not much
  of a panel. The app now understands a small, closed vocabulary under `view` —
  `stat`, `list`, `bars`, `progress`, `clock`, `stack` — claimed by *shape* rather
  than by plugin id, so any plugin gets the product's own components for free:
  a big number, a bar chart, a checklist, a progress meter, or a **live clock** that
  really ticks because a panel with `refresh_ms` is re-read every poll. Every field
  is plain text or a number; lengths, fractions, bar widths and `stack` depth are
  clamped; anything unrecognised keeps the JSON view, so this is additive and an
  older app still reads a newer plugin. Two demo plugins in `plugins/demos/` show
  the whole surface in two single Python files — a **writing desk** that hides the
  Agents tab, the Context button and the composer paperclip and replaces them with
  a live word-count dashboard, and a **workspace atlas** with two panels, its own
  on-disk snapshots, a tool the agent can call, and one action reachable from a
  button, a palette command and `Ctrl/Cmd+Alt+S`. `scripts/verify_plugin_demos.py`
  drives both in a real Chromium against a real server.
- **A plugin can now add controls to the app, not just a tab.** The manifest's new
  `ui` array declares **buttons** in named slots, **command-palette entries**, and
  built-in controls it asks the app to **hide** — all declarative names the app
  owns, so a plugin still ships no markup, CSS or script, and every control is
  drawn by a first-party component. Three slots exist (`composer.toolbar` in the
  composer's action row, `inspector.header` beside **Context**, `session.header`
  before the header chips); commands join the palette as ordinary rows under
  **Plugins**. A control is activated either by `action` — a new `ui/action` RPC
  inside the plugin, answered with a toast, panels to refresh and at most one
  `http`/`https` URL — or by `message`, which posts that text as a user turn. That
  second form also closes an old gap: a panel button could previously only hand
  the agent a message, because no route ran plugin code; now the plugin itself can
  act. Hiding is app-wide and allowlisted (`composer.attach`, the four inspector
  tabs, the inspector's **Context** button), safety controls are deliberately
  absent, and the inspector refuses to hide its last tab instead of drawing an
  empty strip. A contributed control is always attributed — a dot plus a tooltip
  naming the plugin and version — because a plugin must not be able to pass its
  button off as part of the app. The layer is additive: a plugin with no `ui`
  changes nothing, `GET /api/config`'s new `plugin_ui` is empty, an unknown slot or
  hide target from a newer release is skipped with a note rather than failing the
  plugin, and a shape error (no label, two behaviours, a bad id) fails the
  handshake with a fixable message. The SDK exposes it as
  `.button/.button_with/.message_button/.command/.command_with/.message_command/.hide/.on_ui_action`
  over `.ui(…)`; `plugins/example-hello` now contributes all three kinds.
- **Plugins can bind keyboard shortcuts, and every one of their requests is
  reversible by the person using the app.** A fourth contribution kind,
  `keybinding`, binds a chord to the same `ui/action` a button uses — one handler,
  reachable two ways. A binding must carry `mod` (Ctrl/Cmd), `ctrl` or `alt`, so a
  bare key can never shadow typing; the app's own chords (`mod+k`, `mod+b`, `mod+n`,
  `mod+j`, `mod+shift+t`, `mod+enter`) are refused by both host and client, and a
  chord two plugins claim stays with the first in discovery order, so one keystroke
  can never run two plugins. A binding does not fire while the caret is in a field
  or while a dialog is open, and a palette row whose action is also bound shows the
  chord. On the other side of the same bargain, **Settings → Plugins & MCP →
  Controls in the interface** now lists every contributed control per plugin and
  every hidden built-in with the plugin that hid it, with one switch each: a control
  nobody wants can be switched off and a built-in a plugin took away can be put
  back, without disabling the plugin and losing its tools. Those overrides live in
  `localStorage` (per browser, no approval, no round trip) and are applied as a pure
  transform before anything is drawn, so no component needs to know why a control
  is absent.
- **Chat artifacts: a reply can show instead of describe.** A fenced block
  labelled `artifact` — or one whose language is already content (`html`, `svg`,
  `mermaid`, `markdown`, `json`, `csv`/`tsv`, an image) — is drawn in the
  transcript as a card with the thing itself, a **Preview / Source** switch, copy,
  download and a full-screen view. Metadata (`title`, `type`, `height`) rides in
  the info string or a `---`-closed header block. Each kind is drawn at the
  isolation it needs: an HTML mock is a real document in a sandboxed frame with an
  opaque origin (its own inline script and style run, the network does not), an
  SVG is sanitised and drawn inline inside a shadow root so its CSS cannot touch
  the app, Mermaid diagrams are rendered on demand from the app's own palette,
  Markdown reuses the chat's renderer, and JSON/CSV/images are first-party
  components over parsed data. Cards appear when a message settles — a fence that
  is still streaming stays code — and a served document reports its own height so
  a small mock is not a fixed 420px letterbox. Behind it: `POST /api/artifacts`
  (content-addressed, in-memory, 1 MiB) and `GET /api/artifacts/<id>` (route-local
  CSP: `sandbox allow-scripts`, `connect-src 'none'`, no wildcard source). Mermaid
  is the one new runtime dependency and it is a dynamic `import()`: the main
  bundle grows ~5.5 KB gzipped and a chat with no diagram never downloads it. The
  agent is told the convention in its system prompt and in the new `artifacts` doc
  topic, and `scripts/verify_artifacts.py` drives a real Chromium against a real
  server to prove the frame, the policy, the diagram, the table, both themes and a
  390px viewport.
- **A full source-control manager replaces the read-only Changes tab**, in the
  chat area (workspace tab **Source control**) and in the inspector (tab **Git**).
  Changed files are grouped into **Merge changes**, **Staged**, **Changes** and
  **Untracked** — a path with both a staged and a working-tree side appears in
  both, which is git's own model — and every row and every group stages,
  unstages or discards what it holds. The badge letter is repeated as a word, so
  colour is never the only signal. The commit box takes a multi-line message
  (`Ctrl/Cmd+Enter` commits) with an **Amend** toggle, and stages everything
  first when nothing is staged rather than doing nothing. The branch chip opens a
  dialog that filters, switches, creates (and switches to) and deletes local
  branches. The full-width view shows the selected file's patch beside the change
  list with the commit history below it and one commit's files plus patch on
  click; the inspector tab puts the patch inline and loads the history only when
  that section is opened, so opening the app still costs one `git status`. Both
  re-read the tree when they come back into view, and the branch footer now
  reports the same numbers. An untracked file renders a real patch (added lines)
  instead of "no diff". Discarding, deleting an unmerged branch and dropping a
  stash ask for confirmation first, because nothing in the panel can undo them.
- **Git mutations over HTTP**: `POST /api/git/{stage,unstage,discard,commit}`,
  `POST /api/git/{branches,branches/delete,checkout,stash}`, and
  `GET /api/git/{branches,log,commits/:sha,stash}`, each answering with a freshly
  read status so a client never races a second probe against the command it just
  ran. They are local-only (fetch/pull/push stay in the terminal) and keep the
  hardening the read routes already had: argv arrays with no shell, paths
  resolved inside the workspace and passed after `--` with
  `GIT_LITERAL_PATHSPECS=1`, refs and branch names validated against the shared
  `kc_tools::git` grammar, and no repository config able to execute anything on a
  read.

- **Hovering a project with the session list collapsed peeks at that project's
  sessions.** `Ctrl/Cmd+B` gives the rail zero width, which is right for a wide
  conversation and a dead end when all you wanted was to open a session: the only
  way back was to restore the rail, click, and collapse it again. Resting on a
  project tile now shows that project's sessions as a fly-out against the project
  rail — the same rows, the same rename and delete menus, the same fingerprints —
  as a `position: fixed` overlay, so the conversation never resizes to make room.
  Hovering the project already open reads nothing (the store has that list);
  hovering another costs one bounded read of its sessions, cached for 15 s
  (`web/src/state/railPeek.ts`). Opening a row from another project switches
  project first, the fly-out offers **Open this project** (and **New session**
  when it is the project already open), and **Pin the session list open** is the
  same as `Ctrl+B`. It closes when the mouse leaves — after a 260 ms grace period,
  so clipping the edge on the way past does not flash the list — and on Escape,
  which is checked before the Escape that stops a run. It never appears while the
  rail is open, on a phone, in the control plane, or behind an open dialog.
- **Unread notification counters on the project rail.** Each project tile carries
  an iOS-style count of its unread notifications, so a run that finished in a
  project you are not looking at is visible without opening anything. The number
  is in the tile's accessible name and tooltip as well as in the pill, it is
  capped at `99+`, and it clears when the notifications behind it are read.
- **A notification centre**, opened from the bell in the chat header or *Open
  notifications* in the command palette: every notification the server has sent to
  this browser, newest first, with the unread ones marked, the kind spelled out
  (**Finished**, **Needs you**, **Failed**, **Notice**), the body, the session and
  project it came from, and how long ago. Per-row **Mark as read**, **Mark all
  read**, **Clear**, and a click that opens the session behind the row — switching
  project first when it lives in another one. It exists because none of the other
  surfaces persist: a toast is gone in seconds, a desktop notification can be
  refused, and a dot on a session row says *that* something happened without
  saying what.

### Changed

- **A run is one loop: no segments, no `[runtime checkpoint]` turn.** The
  iteration budget used to belong to a *segment* — 200 model turns — and when a
  segment ran out mid-task the runtime wrote a synthetic `user` turn
  (`[runtime checkpoint] Iteration budget refreshed (segment 2/51)…`), pushed it
  into history and carried on. That turn was billed on every later request in the
  session, sat in the transcript as if you had said it, and could read to a model
  as a new instruction — a restart instead of a continuation. `limits.max_iterations`
  now counts turns for the whole run and defaults to **`0`, which means no
  ceiling**: a long task is not interrupted by a counter and nothing is appended to
  the conversation to resume it. `limits.auto_continue` and
  `limits.max_continues` (and `subagents.max_continues`) are gone; a stale
  `config.toml` that still has them loads fine, because unknown keys are ignored,
  and their stored settings rows are simply no longer read.

  What ends a run instead: the model finishing its work, you cancelling it, the
  `max_run_secs` wall clock (24 h), an operator-set `max_iterations` (a plain
  `limit_reached` stop), or the new **loop guard**. The guard counts turns that
  produced *no new information*: every call in them returned an answer the run
  already had, or was refused as a repeat. Four of those in a row stop the run
  with `limit_reached` and a notice saying so, which is what catches a model
  cycling through three questions with the same three answers — a shape the
  per-call repeat rule (which refuses one identical read) never saw. One new
  answer, or any write, resets the count, so fix→test→fix, a growing log tail and
  a file that changed stay legal. Subagents follow the same rules:
  `subagents.max_iterations` also defaults to `0`, and `timeout_secs` is what ends
  one.

  Settings loses the **Auto-continue long runs** toggle and the **Auto-continues**
  fields; **Max iterations** and **Turns per subagent** now say `0 = no ceiling`.
  The system prompt tells the model it has no turn limit, so it does not stop
  early to save one.

  The sheet had to learn the same rule. Its Limits grid validated every field with
  one `min = 1`, so a saved `max_iterations = 0` — the value the server hands back
  and the label calls "no ceiling" — opened with *"Max iterations per run (0 = no
  ceiling) must be a positive integer"* under it and refused to save. Each field
  now carries its own bounds, mirroring `LIMITS_BOUNDS` in the server, the number
  input advertises them (`min`, `max`) so a browser spinner cannot refuse `0`
  either, and a value the server would reject is named with the field's real range
  instead of a second 400 the user cannot act on.

  A run that stops at a limit now says why. `limit_reached` used to mean one
  thing — an iteration budget, "reached a configured limit" — and it now also
  covers the wall clock and the loop guard, which nobody configured. The runtime
  puts its own one-line reason on the finished run (and on the run row), the
  transcript shows that sentence as the notice exactly once, and a reload rebuilds
  it from the row, so *why* a run stopped survives a refresh instead of leaving
  only the word "Limit reached".

- **The working steps start open, and where they start is a setting.** The fold
  covered one finished turn, and every run of calls inside it had to be opened
  again by hand, so **Show the working** stopped one level short of the calls
  themselves. A fourth transcript option — **Working steps** (`working`) — now
  sits beside Tool calls, Grouping and Reasoning in Settings → General →
  Transcript and in the Theme Studio: `open` (the built-in default) shows a
  finished turn's narration, its runs of calls and the calls inside them, with the
  summary line on top as **Hide the working**; `folded` keeps the terse view where
  the work is one **Show the working** line. Where the state lives did not change:
  it is per row — one turn, one run of calls — so expanding one never moves
  another, and switching the option repaints only the rows nobody clicked.
  Stored themes and imported files from before the option keep the folded look
  they were written with; only the built-ins start open.
- **Throughput is measured per block, where you are already looking.** The
  `↓ <tokens> <rate> t/s` badge sat on the assistant turn's header, so the one live
  readout a long run has was scrolled out of view as soon as the turn grew past a
  screen. It now rides the block that is producing text: a streaming **Thought**
  header, the streaming message's own prose (after its caret), and the header of
  every running tool or command card — in both the card and the one-line row style,
  and inside a folded group's open body. Each badge counts the characters of the
  block it sits on (`chars / 4`, the same estimate as before; the tooltip names
  what it counted — prose, reasoning, or a call's output), so a running
  `cargo test` reports the rate its own output is arriving at rather than the
  turn's frozen model total. None of them outlive what they describe: a badge is
  mounted only while its block streams or runs, exactly like the turn-level one,
  and the **Working** chip still speaks for the whole turn.
- **The unread badge, the project counters, the title total and the notification
  centre are derived from one list.** `store.notifications` is the single source
  (`web/src/state/store.ts`); `store.unread`, `store.unreadByProject` and
  `store.unreadTotal` are computed from it, so marking anything read clears all of
  them and they can never disagree. The list is kept in this browser's
  `localStorage` (`karucode.notifications`, capped at 100 entries and pruned after
  a week) because the server stores no notifications — they describe live state.
  The notifier now hands every notification to that list (`NotifierDeps.record`)
  instead of counting an unread total itself; one about the session already on
  screen is recorded as read rather than dropped.
- **`notification` events carry `project_id`.** The project the notifying session
  belongs to, omitted when the notice names no session or the session could not be
  read. The browser only ever holds the open project's sessions, so it cannot work
  out the owner itself — without this field a badge on a project you are not
  looking at is impossible. See the event table in `docs/API.md`.

- **The service runs as the user who installed it.** `install.sh` defaulted
  `SERVICE_USER` to a dedicated `karucode` system account, which meant the agent
  could not reach the person's own toolchains: `~/.local/bin` (first on `PATH`) is
  `700` on many systems, so every command and every terminal tab failed with
  `Permission denied`, and the project directories under `$HOME` were unreadable
  too — the ACL had an entry for a bare uid, not for the service account. The
  default is now the human who ran the installer: `$SUDO_USER`, then `$LOGNAME`,
  then the session login uid (`/proc/self/loginuid`), with `$USER` used only when
  it is not `root` — under `sudo` it always is, which is what made the first cut of
  this change resolve to root and fall through to the system account. Every
  candidate must be an existing, non-root account.

  Root is refused in every spelling, not just the literal string: `--service-user
  root`, `0`, `00`, `root:root` and `0:0` all exit 1, because systemd accepts a uid
  and a `user:group` pair and the old textual check let them through. The account
  is never created and its files are never re-owned, and `--service-user` now
  refuses a name that does not exist instead of letting the unit die later with
  `status=217/USER`.

  The data directory and the group now follow the *account* rather than the
  account's name. `~/.local/share/karucode` for a person; the account's own home
  for a system account, so `--service-user karucode` gives `/var/lib/karucode`
  instead of the nonsensical `/var/lib/karucode/.local/share/karucode`. `Group=`
  comes from the account's real primary gid (resolved through `/etc/group`) rather
  than assuming a group named after the user exists — wrong on any host with
  user-private groups disabled, where systemd then fails with `Failed to determine
  group credentials`. The new `--service-group` overrides it, and `packaging/
  update.sh` forwards the running unit's group instead of assuming `Group=$user`.

  The fallback to the `karucode` system account is now reachable only when *no*
  human can be identified (a bare `sudo -i` shell, a container or image build), and
  it warns loudly instead of happening quietly. Both the per-step output and the
  summary name the resolved account — `Service account: user:group`, plus
  `Data directory:` — so `systemctl show -p User -p Group karucode` can be checked
  against what the install reported. `User=`/`Group=` are now always written to the
  drop-in, and `--dry-run` prints the exact `10-install.conf` it would write.
- **The install is one command and asks nothing.** `install.sh` needed
  `--workspace`, `--data-dir` and (often) `--service-user`, none of which are
  decisions a first-time operator can make confidently. All three are now
  optional: `sudo sh install.sh` installs to `/usr/local`, runs as the account that
  invoked it, and puts its data in that account's `~/.local/share/karucode`. The
  drop-in no longer restates what the unit already carries as a default.
  `--workspace` is gone from the defaults entirely (see below).
- **There is no default workspace.** A fresh install starts with no project: the
  project rail is empty, `GET /api/health` and `GET /api/projects` report
  `workspace: null`, and a session that names no project gets a `400 bad_request`
  telling you to create one. The first project — its directory, and the model
  endpoint and key — is chosen in the web UI. Starting with `--workspace` (or
  `KARUCODE_WORKSPACE`, or `workspace =` in the config file) still adopts that
  directory as the first project. Naming a workspace that does not resolve is
  still fatal: a typo must not silently degrade to "start empty".
  `ProjectRegistry` now holds the implicit workspace as `Option`, and the unused
  `WorkspaceProvider::default_workspace` method is gone from the trait.
- **JIT is on by default.** `MemoryDenyWriteExecute=yes` kills every `node`, Java
  and some Python C extension (V8's JIT needs `mprotect` on executable pages), so
  the agent could not build a frontend or run a JS test suite out of the box. The
  installer now writes `MemoryDenyWriteExecute=no`; `--no-jit` restores the
  stricter setting.

- **The packaged unit no longer sandboxes the filesystem.** `karucode.service` used
  to run the agent under `ProtectSystem=strict` + `ProtectHome=read-only` with a
  `ReadWritePaths=` allowlist, which meant the whole filesystem was read-only to
  the agent except the paths an operator listed. That wall blocked ordinary work —
  `mv`/`cp`/`rm` anywhere outside the workspace failed with `EROFS`, and every new
  toolchain cache (`~/.cargo`, `~/.npm`, `~/.pnpm`, plugin checkouts, screenshot
  directories) needed a new entry that was easy to forget and hard to diagnose when
  it surfaced deep inside a build. The three directives are gone from the unit, and
  `install.sh` no longer writes a `ReadWritePaths=` drop-in (it clears any leftover
  one). `NoNewPrivileges=yes` stays, as does the rest of the kernel/device
  hardening, and the agent still runs as a dedicated unprivileged user — that
  account is the boundary now. To get the old behaviour back, add
  `ProtectSystem=strict`, `ProtectHome=read-only` and `ReadWritePaths=` to
  `karucode.service.d/20-local.conf` yourself; `packaging/README.md` and
  `docs/SECURITY.md` document that recipe, including the `ProtectSystem=no`
  gotcha (an empty `ProtectSystem=` is rejected by systemd and silently ignored).
- **The "Not a sandbox" note is gone from the settings sheet.** It explained that
  `run_command` was unsandboxed and pointed at the systemd unit as the isolation
  story. With the unit no longer sandboxing the filesystem that paragraph read as a
  contradiction, so the Agent tab no longer carries it. The plugin note stays: it
  is about plugin *permissions* being declarations rather than an enforced sandbox,
  which is still true and unrelated to the unit.

### Fixed

- **"Jump to latest" no longer flashes an empty transcript.** The scroll was moved
  to the tail and the windowed transcript only re-read its row set on the next
  animation frame, so for a frame the viewport sat inside the padding of the
  window that had been there before — measured in Chromium: one frame with zero
  rows covering the viewport, after the scroll had already moved 157 000px. The
  window is now read on the scroll event itself, so the scroll position and the
  mounted rows change in the same commit. The jump also **snaps** when it is more
  than three viewports away (`stickyBottom.jumpBehavior`): Chromium covers a
  "smooth" scroll of that length in one or two enormous steps, each mounting a
  screenful of rows it has never drawn, which is a strobe rather than an
  animation. A short jump still animates. Measured after the change: 0 blank
  frames from 4 000px, 120 000px and 2 900px away, a far jump lands directly
  (`distanceFromBottom` 0) and a 200px one still moves in 56px frames, with the
  scroll still holding at 0.5px/0.0px of reader-visible jump.
- **The conversation no longer glitches while it scrolls.** A windowed transcript
  only mounts the rows around the viewport, and it sized its padding from an
  estimate of each row's height. Two things went wrong with that. First, the
  entry animation lived on `.msg`, so every row the window remounted replayed it:
  rows the reader was looking at faded in from transparent and slid 5px as they
  came into view, in both directions. It is now `.msg.is-fresh`, applied only to a
  row that has just joined the conversation (an opening session draws as it is,
  without every row announcing itself). Second, when a row was measured the view
  was moved by the difference between its real height and the *estimate* it
  replaced — but Chromium had already laid that row out at its real height, so the
  correction moved the reader for a layout that never changed. In a browser with
  a 163 000px transcript, a 120px wheel step became 267px of visible jump
  scrolling up and 32px scrolling down. The window is now sized from each row's
  measured *extent* (border box plus its own bottom margin, `content-visibility`
  off inside the window so even the overscan reports a real box), the correction
  is the layout shift itself, anchored on the row at the top of the viewport, and
  the transcript opts out of the browser's native scroll anchoring so the shift is
  not compensated twice. Same measurement: 0.5px up, 0.0px down, nothing fading
  and nothing moving once the input stops. `ChatTranscript.window.test.tsx` pins
  both halves (it fails on the previous code with a 3864px scroll where the 300px
  layout shift was, and with no `.is-fresh` at all).
- **Paging back through a huge session shows the page it just loaded.** A page of
  a long conversation is a window of stored bytes (5000 rows or 1 MiB of text), so
  one page can be tens of thousands of pixels tall. When the reader was at the very
  top — with only the *Scroll up to load earlier messages* line on screen — the
  loader prepended the page below that line and then moved the view down by the
  page's height to "keep the line being read in place". That carried them a whole
  page into history, so the rows that had just arrived were never seen: dragging a
  24 MB session to the top loaded one page per gesture and showed none of them.
  Inside the rows the anchor is still the row being read (the scroll moves by
  exactly the height the page added); at the top the view now moves only far enough
  to put the first row of the new page at the top of the viewport, so each
  scroll-to-top gesture reveals one page from its beginning, with the band above
  the line as the room to scroll into for the next one
  (`web/src/lib/historyLoader.ts`, anchored on the new `content` element - the
  transcript, or an agent's step list - which is also why an opened session still
  lands on its newest content). `scripts/verify_transcript_pagination.py` now
  checks the scroll position after every page arrives, not just that the marker's
  text is somewhere in the document.
- **Collapsing the Results dock no longer shrinks the conversation.** The narrow
  70ch reading measure is a *split* rule — side by side, Conversation is the narrow
  half — but it was applied to the whole centre pane as well, so hiding the dock
  took the transcript and the composer from 820px to 638px at a 1920px viewport (a
  434px empty gutter either side of the text): the opposite of what asking for the
  freed space means. The measure now lives inside the wide container query, and
  every tier that is not a real split — the collapsed dock, the ≤1100px overlay and
  the phone sheet — keeps the app's own content measure, which is also the width of
  the composer box the transcript has to line up with.

### Added

- **Unbounded subagents: `0` now means "no ceiling".** `subagents.max_concurrent`
  was capped at 16 by the settings API and `0` was rewritten to `1` on load, so the
  widest fan-out an operator could ask for was 16 loops — and asking for `0`, the
  natural spelling of "no limit", produced the *narrowest* possible install. Both
  switchers now read `0` as no ceiling: `SubagentLimiter` grants every slot (while
  still counting `active`/`peak`), and `spawn_batch` skips the `max_background`
  capacity check instead of comparing against zero, which had made every
  background spawn a guaranteed refusal. `max_background` follows the same rule,
  `max_depth` keeps its own (`0` = no delegation, `255` = the `u8` ceiling), and
  the bounds are now `0..=1024` for the two counts and `0..=255` for depth, with
  anything above them a `400` that names the field and the range. Depth and time
  still bound a spawn, so what stops a runaway is cost, not a counter.

  The Settings screen had drifted the other way — it accepted up to 32 concurrent
  agents and depth 10, so a value the form allowed came back as a server `400` it
  could not explain. One `SUBAGENT_BOUNDS` table now mirrors the server's, the
  three inputs carry the same `min`/`max`, and each says what zero means. A roster
  that an unbounded fan-out can now fill renders its first 60 rows (live agents
  first) behind a **Show all N** control instead of building hundreds of cards.

- **Steer a running turn, queue a follow-up, and take either back again.** While
  a run is working the composer queues instead of sending, with a **Steer** /
  **Follow-up** toggle that says what the message will do: a steer folds into the
  turn in flight at the loop's next step, a follow-up starts the run after this
  one. Everything queued appears in a panel **above** the box — kind, draft and
  attachment chips — and each row has **Recall**, which pulls the message out of
  the queue (`DELETE /api/sessions/:id/queues/:queued_id`, `404` if the run
  already applied it) and puts the form back in the composer: the draft you
  typed, not the rendered body, and the chips with their inlined text, so sending
  again produces exactly the request the recall took back. A draft written since
  the message was queued is kept, with the recalled text appended below it.
  New: `GET /api/sessions/:id/queues`, `queued_steers` / `queued_follow_ups` on
  `GET /api/sessions/:id`, and a `queued_messages` event published on every queue
  mutation with both lists in full — so a reload, a second tab or a client that
  missed a frame shows the same pending work instead of silently losing it.
  `POST /api/sessions/:id/messages?mode=steer|follow_up` now also accepts
  `draft` and `attachments`, which is what makes a recall restore the form.
  `scripts/verify_queues.py` drives the whole story in a real Chromium (queue a
  steer with an inlined file, queue a follow-up, recall both, re-queue the
  recalled message and compare its rendered body byte for byte, check the panel
  above the box and the phone layout): 50 checks.
- **A Session tab replaces the inspector's Commands tab.** The old tab repeated
  command cards that the transcript already renders inline; it is gone, and in
  its place the inspector describes the session itself. The panel shows the
  main agent's **context window fill** (the newest call's prompt against the
  model's own `context_window`, with no percentage at all when the model entry
  declares none), input / output / cached tokens with an honest **cache hit
  rate** (`Not reported` when the endpoint returned no figure — unknown is not
  0%), an estimated **cost** when the model entry declares per-million prices,
  prompts vs agent turns vs model calls vs tool calls, runs by outcome,
  **working time** (the sum of finished runs, so idle gaps are excluded), the
  per-tool breakdown, one row per **subagent** with its own tokens, cache rate,
  message count and duration, and the newest runs. Figures come from
  `GET /api/sessions/:id/info` rather than the transcript on screen, so they
  cover the whole session and survive a reload.
- **Per-call token accounting.** Every provider call now persists its input,
  output and cached token counts (new `usage_calls` table, schema v8) tagged
  with the agent that made it, which is what makes "what did this session cost,
  and how much of it was the subagents" answerable after a reload. Previously
  the `usage` event was live-only: the numbers existed only in the browser tab
  that happened to be open.
- **`GET /api/sessions/:id/info`** returns a session's counts and totals without
  its transcript: whole-session message counts by role, tool calls by name, runs
  by status together with `working_ms`, per-agent token totals, and the window
  fill. Cheap enough to refresh on a long run.

- **Rewind a conversation to an earlier message, and put the files back.**
  Every user prompt and every agent answer in the transcript now carries a
  **Rewind** button. It sends the conversation back to that message and offers
  three things to do from there: **Just remove it** (the transcript ends at that
  message, ready for you to edit the prompt), **Ask again** (the same request is
  re-sent from a clean conversation) and **Send a note instead** (a different
  instruction replaces it). A separate, off-by-default **Revert** checkbox undoes
  the file changes the agent made after that message - `write_file` and
  `edit_file` now journal the bytes they are about to replace, and the dialog
  lists every affected path with **Restore** or **Remove** beside it, warns when
  a file has no saved before-image (over 4 MiB or not text), and names the tools
  it cannot undo at all (`run_command`, plugins, MCP). Messages are hidden rather
  than deleted, so a rewind never renumbers the transcript a second browser is
  reading. New endpoints: `GET`/`POST /api/sessions/:id/rewind`, a
  `session_rewound` event, and schema v7 (`messages.rewound_at`,
  `checkpoints`/`checkpoint_blobs`). See `docs/API.md` and the `http-api` docs
  topic.
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

### Changed

- The inspector's tab strip is **Files / Changes / Session / Agents**. The
  Session tab's tooltip carries the context fill, because a bare percentage in
  the tab strip would read as a count of something.

### Fixed

- **A turn's thinking survives a reload.** The model's reasoning was streamed to
  the transcript and then dropped: the provider-stream loop forwarded each
  `reasoning_content` delta to the browser but never accumulated it, so
  `Message::assistant_with_reasoning` was always handed `None` and no stored
  assistant row ever carried `reasoning_content` — although the schema (v11),
  `GET /api/sessions/:id` and `docs/API.md` all describe it as stored with the turn
  and sent back on later requests. Switching sessions or reloading the page
  therefore erased every **Thought** block. The stream now keeps the reasoning
  (a cancelled, timed-out or failed attempt keeps what it had already streamed,
  alongside its partial text), and the web client restores it: `seedChatState` and
  the Agents tab's `activityFromMessages` read `reasoning_content` instead of
  hard-coding an empty string, and a turn that only thought and then called a tool
  — no prose at all — is drawn as a thinking block above the tool row rather than
  vanishing from the turn.
- **A busy workspace stays responsive: a streaming answer no longer costs more to
  draw the longer it gets, and open tabs no longer starve the browser.** Three
  separate costs were measured in the shipped bundle while four sessions and their
  subagents were running. First, `Markdown` re-parsed the *whole* message on every
  token delta: a 120 KiB answer cost ~50 ms per render, so the tab sat at 14–20 fps
  and every click - a project switch above all - queued behind a render. A live
  message is now drawn in two halves (`web/src/lib/streamingMarkdown.ts`): the
  blocks that have finished, parsed once and re-parsed only when the boundary
  moves, and the block still being written, re-parsed per delta and highlighted
  only once it closes. The same transcript now runs at 58–59 fps with no long
  tasks, and the finished message renders once, fully, when it settles. The
  reasoning block follows the same rule: a live thought renders its newest 6 000
  characters instead of re-laying out the whole thing per token, and it stops
  pulling the scroller back to the bottom once you have scrolled up to read.
  Second, every tab opened its own `/api/events` stream. Six tabs pinned all six
  of the browser's HTTP/1.1 connections to the origin, so the seventh could not
  boot at all - *Could not reach karucode · The server did not answer within 30s*
  on `/api/health` - while `curl` to the same server answered in 1.6 ms. One tab
  now holds the stream under a Web Lock and relays every frame to its siblings
  over a BroadcastChannel (`web/src/api/sharedStream.ts`); with ten tabs open a
  new one boots immediately and health checks answer in single-digit
  milliseconds. Third, the whole project's agent list - several megabytes on a
  project with history - was re-read on every session-list refresh, including
  every `run_started`/`run_finished`. It is now re-read when a session is added,
  removed or renamed, on a project switch, and when the Agents tab asks for it.
- **Token totals are one row per provider call again, and a running subagent's
  iteration count is real.** Two numbers in the session view were wrong for
  reasons outside the view. A gateway that puts the `usage` object on every
  streamed chunk had each chunk accounted as its own call: one 30-minute run wrote
  ~70k rows, so the session's totals read roughly 100× the real spend and the call
  count was one that never happened. The attempt now keeps only the newest `usage`
  block and publishes and persists it once, when the call ends, so the documented
  contract - one row per provider call (`GET /api/sessions/:id/info`) - holds
  whatever the endpoint's chunking. Separately, `iterations` was live-only: the
  agents row carried its count only once the agent finished, so a reload, a
  refresh or a second tab showed `0` for every running subagent. The loop now
  writes the count in place as it advances (`MAX`, so a stale write cannot run it
  backwards), and a fetched row that lags the live progress frames can no longer
  lower the number the page is already showing.
- **The resident set no longer ratchets with use.** A server that had served a
  few long runs and six plugins sat at roughly twice the memory it does when it
  starts, and kept climbing (`105 MB → 153 MB` over an afternoon on the machine
  this was measured on) while holding almost nothing: of 87 MiB of anonymous
  pages, most were glibc's per-thread arenas — up to `8 * nproc` separate heaps,
  one per thread that ever allocated — holding *freed* memory that glibc does not
  hand back to the kernel. The binary now caps the arena count
  (`KARUCODE_MALLOC_ARENA_MAX`, default 2) before the runtime starts, and returns
  free heap to the kernel while idle (`KARUCODE_MALLOC_TRIM_SECS`, default 60).
  Measured with `scripts/measure_steady_memory.py`, six real plugins and twelve
  concurrent runs, two rounds each: idle 46.3 MiB → 31.4 MiB (anonymous pages
  30.9 MiB → 15.7 MiB); on a production-shaped workload (a 350 MB database and
  eight turns that each return a 900 KB tool result) the old build stayed at
  41 MiB after the run and the new one settled back to 22 MiB. Plugin processes
  are unaffected — their own RSS is their own.

- **The session list's rows are laid out as one line again.** The Session panel
  named its definition list `.session-meta`, which is the sessions rail's own
  class for a row's timestamp and message count; because every stylesheet is
  imported into one flat bundle and `inspector.css` comes later, the panel's
  `flex-direction: column` won and every row stacked "13m ago" over "832
  messages" in the centre. The panel now uses its own `si-` prefix, and
  `web/src/styles/inspector.test.ts` fails if it ever reaches for a `session-*`
  class again.

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
