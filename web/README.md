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

For the Rust server, either let it serve the embedded build or point it at this
directory's `dist`:

```bash
KARUCODE_WEB_DIR="$PWD/web/dist" cargo run -p kc-server
```

The dev proxy uses `changeOrigin: false`, so the browser `Origin` and forwarded `Host`
stay on the Vite port and pass the server's CSRF origin/host check. In production the
UI and API are same-origin.

## Structure

```
src/
  api/        typed client, endpoint functions, fetch-based SSE stream, API types
  state/      signals store, pure event reducer, session→transcript seeding, actions
  components/ rail, transcript, cards, inspector tabs, settings, dialogs, states
  lib/        markdown+DOMPurify, highlight.js registry, diff parser, format, focus
  styles/     tokens, base, layout, chat, inspector, overlays, settings
  test/       jsdom setup, fixtures, smoke render
```

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
- Inspector: lazy file tree, line-numbered highlighted file viewer, git status/diff,
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

## Testing

`npm test` covers:

- the pure event reducer (streaming segments, commands, approvals, errors,
  cancellation, subagents, session list, seeded history, unknown/other-session
  events);
- Markdown sanitisation (`<script>`, `<img onerror>`, `javascript:` links) and
  fenced-code labels/copy/highlighting;
- composer send/stop/disabled/counter/live-region behaviour;
- approval decisions and resolved/expired states;
- session list rendering, inline rename, delete confirmation and empty/loading
  states;
- mocked SSE parsing, split chunks, keepalives, seq dedupe, backoff and 401 handling;
- diff parsing;
- a JSDOM smoke render of the whole app against mocked documented endpoints, which
  asserts zero `console.error`/`console.warn` output.

Latest production build (gzip):

- JS: ~120 KB
- CSS: ~10 KB