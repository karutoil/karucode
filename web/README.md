# karucode web

The browser workspace for karucode: sessions, a streaming agent transcript, approvals,
a workspace/git/commands/subagent inspector, and a settings sheet. Built against
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
              Theme Studio, settings, dialogs, states
  theme/      theme engine, built-in palettes, colour maths, storage/import/export
  lib/        markdown+DOMPurify, highlight.js registry, diff parser, format, focus,
              @-mention index and ranking, attachment classification/formatting
  styles/     tokens, base, layout, rail, chat, composer, inspector, overlays,
              settings, theme-studio
  test/       jsdom setup, fixtures, smoke render
```

## Theming

`styles/tokens.css` is the only place raw colours live. It declares a
`--theme-*` block plus public aliases (`--surface`, `--line`, `--accent`, …) and
the frame metrics (`--rail-w`, `--header-h`, `--content-max`, …).
`src/theme/engine.ts` rewrites the `--theme-*` block and sets `data-theme`,
`data-mode` and `data-density` on `<html>`, so every component repaints from CSS
custom properties with no React/Preact re-render.

Theme Studio (project-rail palette button, header theme button,
`Ctrl/Cmd+Shift+T`, or Settings → Appearance) ships eight built-in themes and
edits 18 palette colours, mode, density, fonts, corner radius and custom CSS.
The selection, saved custom themes and the unsigned draft persist in
`localStorage` under `karucode.theme-studio.v3`; a theme exports as
`karucode-theme` version 1 JSON and imports back through a validating parser
(150 KB cap, hex/mode/density/font checks). See the `themes` topic in the
`karucode_docs` tool for the user-facing version.

## Attachments

Small text files (≤ 64 KiB, no NUL byte in the first 8 KiB) are read in the
browser and inlined into the message as a fenced block; everything else is
uploaded to `.karucode/attachments/<session>/` with
`POST /api/workspace/upload` and referenced by its workspace path. The server
owns the limits (8 MiB, single path segment, no overwrite) and the UI mirrors
them before making a request.

The API client is the only place that knows about headers/JSON/errors. The event
stream is a standalone function so reconnect/dedupe can be tested with a fake
`fetch`. The reducer (`state/eventReducer.ts`) is pure and ignores unknown event
types, other sessions' events, and subagent text/tool deltas (those belong to the
Agents tab).

## Included behaviour

- Sessions: create, inline rename, delete confirm, running/failure dots, message
  counts, relative time, empty state, Cmd/Ctrl+N.
- Chat: streamed Markdown (raw HTML off + DOMPurify), code labels/copy/highlighting,
  collapsed reasoning, tool cards with durations/truncation, streaming command
  cards with stdout/stderr/exit codes, blocking approval cards, per-run error
  banners, interrupted notices.
- Composer: auto-grow, Cmd/Ctrl+Enter send, Stop/Esc, disabled explanations,
  character counter after 4000, live activity status.
- Composer: `@`-mentions over a real workspace index (files only, ranked by
  name → path → subsequence, keyboard-navigable), attachments from the picker,
  drag-and-drop or paste, inline/upload chips with per-file status, and a send
  that is blocked while a file is still reading or uploading.
- Themes: eight built-in palettes, live editing of 18 colours, dark/light mode,
  three densities, interface/code fonts, corner radius, custom CSS, contrast
  readout, save/delete/import/export and per-browser persistence.
- Workspace tabs: Conversation, and Changes rendered full width from the same
  git data as the inspector (no terminal view — the server has no shell API).
- Sessions: client-side session search (`/`), sessions grouped by day, running
  and failure state.
- Inspector: Files / Changes / Commands / Agents tabs, lazy file tree,
  line-numbered highlighted viewer, git status/diff with a branch footer,
  per-session commands, live subagent cards.
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
9. **History size**: there is no message pagination endpoint. The UI renders the
   session detail it receives (the server caps history) and labels truncated command
   output from the server's `truncated` flag.
10. **Plugin/MCP status** is read-only in the UI by design; no enable toggles are
    exposed even though `POST /api/plugins/:name/enable` exists.
11. **Auth token**: stored in `localStorage` (`karucode.token`) and sent as a bearer
    header, including on the SSE fetch. `?token=` is not used. A 401 anywhere shows
    the token entry screen.
12. **Settings lock**: `settings_locked` disables the form, and a 403 from PATCH is
    surfaced as a locked notice rather than a generic failure.
13. **Sequence resets**: `seq` is assumed to be per-server-process/connection; the
    reducer does not enforce global monotonicity across reconnects.
14. **Health/provider state**: `provider_configured` from `/api/health` is the source
    of truth for the composer gate; saving settings triggers a health refresh.
15. **Session ordering**: the UI preserves `/api/sessions` order and selects the
    last-opened session from local storage when it still exists, otherwise the first
    item.
16. **Attachment upload**: `POST /api/workspace/upload?session=&name=[&project=]`
    takes the raw bytes as the request body (`application/octet-stream`, 8 MiB) and
    returns `{ session, name, path, size, content_type, workspace, deduplicated }`.
    The UI always uses the returned `path` (a collision renames the stored file)
    and never the local `File.name`. The message itself still carries text, so a
    binary attachment is referenced by path and opened by the agent's own tools.
17. **Mention index**: `GET /api/workspace/tree?depth=6` is fetched once per
    project (2 000-entry server cap, noise directories skipped server-side) and
    cached for the composer's lifetime; a mention can only name a path the agent
    can actually open.
18. **Theme persistence** is per-browser `localStorage`, not the config file: a
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
- composer send/stop/disabled/counter/live-region behaviour;
- approval decisions and resolved/expired states;
- session list rendering, inline rename, delete confirmation and empty/loading
  states;
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