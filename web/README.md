# karucode web

The browser workspace for karucode: sessions, a streaming agent transcript, approvals,
a workspace/git/session/subagent inspector, and a settings sheet. Built against
[`docs/API.md`](../docs/API.md) and [`docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md).

## Stack

- Vite 6 + Preact 10 + TypeScript (strict, no `any` in public interfaces)
- `@preact/signals` for small, colocated state
- `markdown-it` (raw HTML disabled) + DOMPurify + selected highlight.js languages
- Hand-written CSS with design tokens; no UI/CSS framework and no downloaded webfonts
- Vitest + @testing-library/preact + jsdom for tests

`@types/markdown-it` is a types-only dev dependency. `.npmrc` sets `include=dev` so the
toolchain installs on machines where npm defaults to `--omit=dev`.

## Commands

```bash
npm install
npm run dev        # http://127.0.0.1:5173, proxies /api to http://127.0.0.1:8787
npm run typecheck
npm test
npm run build      # writes dist/index.html + hashed assets
npm run preview    # static preview; also proxies /api to 8787
npm run preview:api  # prints the KARUCODE_WEB_DIR hint for serving the Rust binary
```

`cargo build` does the `npm run build` for you: `crates/kc-server/build.rs`
rebuilds `dist/` when these sources are newer than it, and the result is embedded
in the binary. So `cargo run -p kc-server` serves this UI with no extra flags.

To serve a build you just made without recompiling (useful while iterating), point
the server at the folder:

```bash
KARUCODE_WEB_DIR="$PWD/web/dist" cargo run -p kc-server
```

`npm ci` is part of that flow; skip it with `KARUCODE_SKIP_WEB_BUILD=1` when you
only touched Rust, or hand a bundle to the build with `KARUCODE_WEB_DIST=<dir>`.

The dev proxy uses `changeOrigin: false`, so the browser `Origin` and forwarded `Host`
stay on the Vite port and pass the server's CSRF origin/host check. In production the
UI and API are same-origin.

## Structure

```
src/
  api/        typed client, endpoint functions, fetch-based SSE stream, API types
  state/      signals store, pure event reducer, session→transcript seeding, actions
  components/ rail, sidebar, transcript, cards, inspector tabs, composer,
              Theme Studio, settings, dialogs, states; git/ is the source-control
              manager
  theme/      theme engine, built-in palettes, colour maths, storage/import/export
  lib/        markdown+DOMPurify, highlight.js registry, diff parser, format, focus,
              @-mention index and ranking, attachment classification/formatting,
              dock-height measuring for the floating composer
  styles/     tokens, base, layout, rail, chat, composer, inspector, git,
              overlays, settings, theme-studio
  test/       jsdom setup, fixtures, smoke render
```

## Theming

`styles/tokens.css` is the only place raw colours live. It declares a
`--theme-*` block plus public aliases (`--surface`, `--line`, `--accent`, …) and
the frame metrics (`--rail-w`, `--header-h`, `--content-max`, …).
`src/theme/engine.ts` rewrites the `--theme-*` block and sets `data-theme`,
`data-mode`, `data-density`, `data-tool-style`, `data-grouping` and
`data-thinking` on `<html>`, so every component repaints from CSS custom
properties with no React/Preact re-render.

Theme Studio (project-rail palette button, header theme button,
`Ctrl/Cmd+Shift+T`, or Settings → General → Appearance) ships eight built-in
themes and edits 18 palette colours, mode, density, fonts, corner radius, the
transcript view options and custom CSS. The selection, saved custom themes and
the unsigned draft persist in `localStorage` under `karucode.theme-studio.v3`; a
theme exports as `karucode-theme` version 1 JSON and imports back through a
validating parser (150 KB cap, hex/mode/density/font checks). See the `themes`
topic in the `karucode_docs` tool for the user-facing version.

### Transcript view options

A theme also decides how much of the agent's work the conversation shows. The
rules live in `src/state/transcriptLayout.ts` as pure functions over
`ChatItem[]` (`splitTurns`, `isNarration`, `turnRows`, `turnFoldable`,
`foldTurn`), so they are unit-tested without a DOM and `ChatTranscript.tsx` only
renders what they return.

| Option | Values | Effect |
| --- | --- | --- |
| `toolStyle` | `cards` (default), `rows` | `rows` is one ~26px line per call (`ToolCard variant="row"`) that expands into the same numbered output. |
| `grouping` | `none` (default), `tools`, `turns` | `tools` folds a run of *consecutive* calls into one summary line (`ToolGroup`); `turns` also folds a finished turn's steps behind a `.process-fold` button. |
| `thinking` | `collapsed` (default), `preview`, `expanded` | `preview` shows the first line of reasoning under a `Thought` label; `expanded` renders it open. A streaming model is always open. |

The invariant that makes folding safe is enforced in `turnFoldable` and
`foldTurn`: a streaming turn, a running call, a pending approval, an error, an
interrupted run and a notice are never hidden, and a notice or error that fell
before the answer is lifted out of the fold instead of being folded with the
work. Assistant prose between tool calls is narration (`.narration`) and is
always drawn quieter than the answer, in every view.

## Attachments

Small text files (≤ 64 KiB, no NUL byte in the first 8 KiB) are read in the
browser and inlined into the message as a fenced block; everything else is
uploaded to `.karucode/attachments/<session>/` with
`POST /api/workspace/upload` and referenced by its workspace path. The server
owns the limits (8 MiB, single path segment, no overwrite) and the UI mirrors
them before making a request.

## Chat artifacts

A fenced block in a reply can be drawn as **live content** instead of code: an
HTML mock, an SVG, a Mermaid diagram, a rendered Markdown document, pretty-printed
JSON, a CSV/TSV table, or an image. The user-facing convention (and what an agent
should write) is the `artifacts` topic in `karucode_docs`; this section is how the
renderer is put together.

```
lib/artifacts.ts             pure parser: fence info string + `---` header block -> ArtifactSpec
lib/markdown.ts              renderMarkdownDocument(): the same markdown-it+DOMPurify
                             pipeline, but a recognised fence becomes an empty
                             `<div class="artifact-slot" data-artifact-index=N>`
components/Markdown.tsx      fills those slots with a card (preact `render()` into each
                             element - see below)
components/ArtifactCard.tsx  the card: head (kind, title, Preview/Source, copy,
                             download, expand) plus one renderer per kind
lib/artifactPreview.ts       POST the source once per (source, theme); remember the id
lib/mermaid.ts               the only runtime dependency added: a dynamic `import()`
```

**Why the cards are mounted, not rendered.** The prose and the fences arrive as one
HTML string (that is what markdown-it produces), so the renderer leaves a
placeholder per artifact and `Markdown` calls Preact's `render()` into each
placeholder once the DOM exists. Preact ignores the children of a
`dangerouslySetInnerHTML` element, which is what makes the string and the component
tree safe to mix this way; re-tokenising the Markdown to interleave components
would mean rendering every message twice.

**Each kind is drawn at the isolation it deserves.**

| Kind | How | Why |
| --- | --- | --- |
| HTML | a real document at `/api/artifacts/<id>` in `<iframe sandbox="allow-scripts">` | `srcdoc` inherits this app's CSP (`default-src 'self'`) *and* runs in an opaque origin where `'self'` matches nothing, so its inline `<style>`, `<script>` and relative assets are all refused - measured in Chromium, the preview came up nearly blank |
| SVG | sanitised by DOMPurify, drawn inline in a **shadow root** | an SVG can carry a `<style>` whose selectors apply to the whole document; the shadow root keeps its CSS to itself in both directions |
| Mermaid | rendered by mermaid (`securityLevel: 'strict'`, `htmlLabels: false`), then sanitised again | `htmlLabels: true` wraps labels in `foreignObject`, which the sanitiser removes - the diagram then renders as empty boxes |
| Markdown | the chat's own renderer | one sanitiser, one look |
| JSON, CSV/TSV, image | first-party components over parsed data | a table is DOM nodes, not markup |

**Streaming.** `ChatTranscript` passes `artifacts={!item.streaming}`. markdown-it
closes a fence at the end of its input, so without that a half-written mock would
paint a frame, reload it and relayout the transcript on every token; cards appear
when the message settles.

**Sizing.** The served document carries one injected script that posts its own
content height to the parent; the card clamps it (80-720px) and sizes the frame.
Without it a small mock sat in a fixed 420px letterbox. A complete document keeps
its own CSS - the reporter is the only thing added to it.

**Cost.** The main bundle grows by ~5.5 KB gzipped (parser, card, previews).
Mermaid is *not* in it: `lib/mermaid.ts` is the only module that imports it, by
`import()`, so it arrives as its own chunk on the first diagram in a transcript -
about 153 KB gzipped for the core plus ~20 KB per diagram type - and a chat with no
diagram never downloads it.

`scripts/verify_artifacts.py` is the proof that none of this is only in theory: it
drives a real Chromium against a real server and measures the frame, the served
policy, the SVG, the diagram, the table, both themes and a 390px viewport.

The API client is the only place that knows about headers/JSON/errors. The event
stream is a standalone function so reconnect/dedupe can be tested with a fake
`fetch`. The reducer (`state/eventReducer.ts`) is pure and ignores unknown event
types, other sessions' events, and subagent text/tool deltas (those belong to the
Agents tab).

## Included behaviour

- Sessions: create, inline rename, delete confirm, running/failure dots, message
  counts, relative time, empty state, Cmd/Ctrl+N (ignored while the focus is in an
  input, textarea or contenteditable, so it never steals a text-editing key).
- Chat: streamed Markdown (raw HTML off + DOMPurify), code labels/copy/highlighting,
  collapsed reasoning, tool cards with durations/truncation, streaming command
  cards with stdout/stderr/exit codes, blocking approval cards, per-run error
  banners, interrupted notices.
- Composer: auto-grow, Cmd/Ctrl+Enter send, Stop/Esc, disabled explanations,
  character counter after 4000, live activity status. It is a floating dock over
  the transcript (one line at rest, translucent and footnote-less while you read
  history), reserving its measured height via `--composer-h` — see
  `lib/dockHeight.ts` and `styles/composer.css`.
- Composer: `@`-mentions over a real workspace index (files only, ranked by
  name → path → subsequence, keyboard-navigable), attachments from the picker,
  drag-and-drop or paste, inline/upload chips with per-file status, and a send
  that is blocked while a file is still reading or uploading.
- Themes: eight built-in palettes, live editing of 18 colours, dark/light mode,
  three densities, interface/code fonts, corner radius, custom CSS, contrast
  readout, save/delete/import/export and per-browser persistence.
- Workspace tabs: Conversation, Source control (the git manager, full width) and
  Terminal when the server enables it.
- Sessions: client-side session search (`/`), sessions grouped by day, running
  and failure state.
- Source control: the git manager in two shapes — the full-width workspace tab
  (change list beside the patch, history below it) and the inspector's Git tab
  (one column, patch inline). Files are grouped into Merge changes / Staged /
  Changes / Untracked, each row and each group stages, unstages and discards, the
  commit box commits or amends (`Ctrl/Cmd+Enter`, `stage_all` when nothing is
  staged), the branch chip opens a filter/create/switch/delete dialog, and the
  history shows commits with their files and patch. Progress and errors are real:
  every mutation returns the fresh status, destructive actions ask first, and
  fetch/pull/push stay in the terminal. `components/git/` holds the pieces
  (`GitManager`, `GitChanges`, `GitCommitBox`, `GitHistory`, `GitDiffPane`,
  `GitBranchDialog`, `useGitRepo`, pure `gitModel`), styled by `styles/git.css`.
- Inspector: Files / Git / Session / Agents tabs, lazy file tree, and the
  file viewer (below), a Session tab with
  the session's counts and token totals (context fill, input/output/cached with
  a cache hit rate, prompts vs turns vs model calls vs tool calls, runs and
  working time, per-subagent usage, recent runs), and an Agents console
  (project- or session-scoped, status filters, live per-agent transcripts with
  drill-in).
- File viewer: Markdown and HTML open on a rendered preview with a Preview/Source
  toggle (HTML runs in a fully sandboxed iframe — no scripts, no same-origin
  access, its own CSP), images and PDFs render from `GET /api/workspace/raw`, and
  any complete text file can be edited and saved with `PUT /api/workspace/file`.
  `Ctrl/Cmd+S` saves, the Preview follows the draft while you type, a save whose
  `expected_size` no longer matches offers *Overwrite anyway* instead of
  clobbering the agent's work, and closing with unsaved changes asks first.
  Binary and server-truncated files are read-only and the Edit button says why.
- Settings: provider fields, write-only API key with clear, provider test, approval
  mode/allowlists/timeouts, limits, subagent controls, read-only plugin/MCP status,
  lock notice and the shell-sandbox security note.
- SSE: one fetch stream, bearer header, exponential backoff + jitter, per-connection
  seq dedupe, session refetch after reconnect, unknown events ignored.
- States: boot skeleton, empty/no-session, provider missing, disconnected, 401 token
  entry, 404 recovery, non-blocking error toasts.
- Keyboard/a11y: focus-trapped dialogs, tabs with arrow keys, roving-tree keyboard
  navigation, labelled controls, live regions, visible focus rings, reduced-motion
  support.

## API assumptions and contract notes

The server implementation was landing in parallel; these are the points where the
UI had to make a choice beyond the literal contract. They are worth reconciling with
`kc-server`:

1. **SSE resume**: the contract has no `Last-Event-ID`/replay semantics. The client
   dedupes `seq` within one connection (resetting on reconnect, as specified) and
   refetches the open session after every reconnect. The boot connection does not
   refetch twice.
2. **Global stream**: the UI opens `/api/events` without `?session=` so the rail can
   react to other sessions' lifecycle events, then filters transcript events by
   `session_id` in the reducer. `?session=` remains available if the server prefers
   it later.
3. **Session detail approvals**: `GET /api/sessions/:id` is documented with an
   `approvals` array; the UI treats only `status: "pending"` records as actionable
   and ignores expired/resolved ones on load.
4. **Stored tool results** carry no success flag; command stdout/stderr/exit
   codes are encoded in the tool message's report text rather than separate
   columns. Restored command cards parse the stable report header/sections to
   recover the exit code, duration and streams; if the text does not match, the
   raw report is shown. Stored messages also have no reasoning or usage fields,
   so those appear live only.
5. **Git status granularity**: the contract has one `staged` boolean per file. A file
   with both staged and unstaged edits cannot be represented twice; the UI groups by
   the flag it receives. (The server's porcelain parser also emits one row per path.)
6. **Workspace tree depth**: the response is a flat `entries` array. The UI requests
   `depth=1`, treats entries as children of `path`, and lazy-loads each directory.
   It does not rely on depth>1 flattening. The explorer also filters `.git`,
   `node_modules` and `target` client-side so a listing that is not yet
   ignore-aware still stays usable.
7. **Provider test** probes the persisted server configuration, not unsaved form
   values. The settings sheet says so next to the button.
8. **Approval timeout/404**: a 404 on decision marks the card expired; the server's
   `approval_resolved` event is still authoritative when it arrives.
9. **History size**: there is no message pagination endpoint. `GET /api/sessions/:id`
   returns the *newest* window (`messages_limit`, default 5 000) and sets
   `messages_truncated`; when it is set the transcript opens with a notice —
   "Showing the last N messages; earlier history is not loaded." — instead of
   silently starting mid-conversation. Truncated command output is labelled from
   the server's `truncated` flag.
10. **Plugin/MCP status** is read-only in the UI by design; no enable toggles are
    exposed even though `POST /api/plugins/:name/enable` exists.
11. **Auth token**: stored in `localStorage` (`karucode.token`) and sent as a bearer
    header, including on the SSE fetch. `?token=` is not used. A 401 anywhere shows
    the token entry screen.
12. **Settings lock**: `settings_locked` disables the form, and a 403 from PATCH is
    surfaced as a locked notice rather than a generic failure.
13. **Settings tabs**: the sheet is five tabs (General / Models / Agent /
    Notifications / Plugins & MCP) over one shared form and one shared Save.
    Panels are mounted together and merely `hidden` — a panel that unmounted
    would discard what was typed, and the Models tab owns local state (key
    drafts, the add-provider form). Because the Save is shared, a rejected field
    can be on another tab, so `FIELD_LOCATIONS` maps every validated key to its
    tab, its Advanced disclosure and its input id: a failed save switches tab,
    opens the disclosure and focuses the field, and the footer summarizes the
    count and the tabs involved. Notification switches are the exception — they
    PATCH immediately and have no Save of their own.
14. **Sequence resets**: `seq` is assumed to be per-server-process/connection; the
    reducer does not enforce global monotonicity across reconnects.
15. **Health/provider state**: `provider_configured` from `/api/health` is the source
    of truth for the composer gate; saving settings triggers a health refresh.
16. **Session ordering**: the UI preserves `/api/sessions` order and selects the
    last-opened session from local storage when it still exists, otherwise the first
    item.
17. **Attachment upload**: `POST /api/workspace/upload?session=&name=[&project=]`
    takes the raw bytes as the request body (`application/octet-stream`, 8 MiB) and
    returns `{ session, name, path, size, content_type, workspace, deduplicated }`.
    The UI always uses the returned `path` (a collision renames the stored file)
    and never the local `File.name`. The message itself still carries text, so a
    binary attachment is referenced by path and opened by the agent's own tools.
18. **Mention index**: `GET /api/workspace/tree?depth=6` is fetched once per
    project (2 000-entry server cap, noise directories skipped server-side) and
    cached for 60 s; switching project invalidates it immediately, so a mention can
    only name a path the agent can actually open.
19. **Theme persistence** is per-browser `localStorage`, not the config file: a
    theme does not follow the user to another machine, and the API knows nothing
    about it.

## Testing

`npm test` covers:

- the theme engine: palette resolution, contrast maths, persistence against
  corrupt/oversized storage, import validation, and the Theme Studio's live
  colour/density/mode/save/delete behaviour;
- `@`-mention parsing, ranking and insertion (including quoted paths) and the
  attachment classifier, inline formatting and upload fallback;
- the pure event reducer (streaming segments, commands, approvals, errors,
  cancellation, subagents, session list, seeded history, unknown/other-session
  events);
- Markdown sanitisation (`<script>`, `<img onerror>`, `javascript:` links) and
  fenced-code labels/copy/highlighting;
- composer send/stop/disabled/counter/live-region behaviour, plus the floating
  dock: it sits over the transcript rather than in its own row, goes translucent
  when you scroll into history and solid again on focus, and publishes its
  measured height so the transcript reserves room for it (`styles/composer.test.ts`,
  `lib/dockHeight.test.tsx`, `components/ChatView.test.tsx`);
- approval decisions and resolved/expired states;
- session list rendering, inline rename, delete confirmation and empty/loading
  states;
- the settings sheet: the five tabs and their panels, arrow-key tab movement,
  typed values surviving a tab switch, Advanced blocks starting collapsed, and a
  rejected field pulling the sheet to its tab and disclosure — plus the
  stylesheet contract in `styles/settings.test.ts` that keeps the scroll on the
  panel rather than the sheet body;
- mocked SSE parsing, split chunks, keepalives, seq dedupe, backoff that grows
  across reconnect attempts, the 45 s silence watchdog, `seq`-gap and
  `stream_lagged` reporting, a handshake deadline, and 401 handling;
- diff parsing;
- a JSDOM smoke render of the whole app against mocked documented endpoints, which
  asserts zero `console.error`/`console.warn` output.

Latest production build (gzip):

- JS: ~158 KB
- CSS: ~23 KB

The growth over the previous build is the Theme Studio (8 palettes, colour maths,
import/export), the 114-role fidelity layer that keeps the UI pixel-identical to
its design mock across all eight themes, the mention index and the attachment
pipeline; no runtime dependency was added.