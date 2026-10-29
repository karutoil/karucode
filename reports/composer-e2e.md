# Composer: mentions and attachments, verified against a real server

Manual end-to-end check for the composer work (`web/src/components/Composer.tsx`,
`FileMentionMenu.tsx`, `web/src/lib/{mentions,attachments}.ts`,
`web/src/api/{client,endpoints,types}.ts`, `web/src/styles/composer.css`).
Automated coverage lives in the three vitest suites listed at the bottom.

## Setup

```bash
cd /home/karutoil/karucode/web && npx vite build          # dist is served by the binary
mkdir -p /tmp/kce2e/ws/src /tmp/kce2e/data
printf '# Demo\n' > /tmp/kce2e/ws/README.md
printf 'export const answer = 42;\n' > /tmp/kce2e/ws/src/main.ts
KARUCODE_WORKSPACE=/tmp/kce2e/ws KARUCODE_DATA_DIR=/tmp/kce2e/data KARUCODE_PORT=8795 \
  KARUCODE_WEB_DIR=$PWD/../web/dist target/debug/karucode
```

The page is then driven over CDP (`chromium --headless=new --remote-debugging-port=9222`)
so the checks go through the real Preact app, the real store and the real HTTP API.

## What was checked

1. **Session gate** — a fresh profile has no session, so the composer is disabled
   with "Create or select a session to start." (unchanged behaviour).
2. **`@` menu** — typing `look at @main` opens the popover and offers exactly
   `["src/main.ts"]` from `GET /api/workspace/tree?depth=6`. Geometry measured in
   the live page: composer box `x=349 w=722`, popover `x=350 w=720` (inset by the
   1px border), background `rgb(34,35,38)` (opaque), `opacity: 1`, radius 10px,
   popover bottom 7px above the box. `aria-expanded="true"`,
   `aria-controls="composer-mention-menu"`,
   `aria-activedescendant="composer-mention-menu-option-0"`.
3. **Keyboard insert** — `ArrowDown` highlights the first option, `Enter` is
   prevented and the draft becomes `look at @src/main.ts ` (trailing space, caret
   after it).
4. **Drag and drop** — dispatching `dragenter` with a `DataTransfer` renders the
   `.composer-drop` overlay ("Drop files to attach"); `drop` with
   `notes.md` (`# Note\nhello from a dropped file`, 32 B) produces a chip reading
   `Inlined · 32 B` with `data-mode="inline"`.
5. **Binary upload** — dropping a 12-byte PNG produces a chip reading
   `Uploaded · 12 B` with `data-mode="upload"`; the server wrote
   `.karucode/attachments/<session>/diagram.png`.
6. **Payload** — clicking Send cleared the draft and the chips, and
   `GET /api/sessions/<id>` shows the stored user message:

   ```
   summarise what I attached

   Attached files:

   ### notes.md (inlined, 32 B)

   ```md
   # Note
   hello from a dropped file
   ```

   ### .karucode/attachments/<session>/diagram.png (uploaded, 12 B)

   Read this file from the workspace when you need it: `.karucode/attachments/<session>/diagram.png`
   ```
7. **Upload endpoint contract** (the locked one the client speaks):
   `POST /api/workspace/upload?session=<id>&name=shot.png` with `X-Karucode-Client:
   web`, `Content-Type: application/octet-stream` and the raw bytes → `201`
   `{"path":".karucode/attachments/<id>/shot.png","name":"shot.png","size":12}`
   (the server also sends `session`, `content_type`, `workspace` and
   `deduplicated`; the client type marks those optional and relies only on
   `path`/`name`/`size`). Posting the same name again returns `"name":"shot-1.png"`,
   which is why chips compose the message reference from the response and never
   from `File.name`. Bytes survive verbatim (`binary\0payload` round-trips
   byte-for-byte through the store), the uploaded path appears in the tree
   listing so it is mentionable immediately, and `GET /api/workspace/file` reads
   it back.

## Visual pass

The production CSS bundle was rendered in headless Chromium for the empty
composer, the chip row (inlined / uploaded / uploading / failed), the open
`@`-menu and the drop overlay, at 1440px and 390px, next to
`karucode-v3-themes.html`. Structure follows the mock: `+` button on the left of
the textarea, one quiet bottom row with model/reasoning pills and Send, and a
footnote line carrying the status live region plus the shortcut hints.

One caveat when re-running this: headless Chromium freezes CSS animations, so the
`mention-in` entrance animation can be screenshotted at `opacity: 0`. Force
`animation: none` on `.mention-popover` for a faithful capture (that is also what
`prefers-reduced-motion: reduce` does).

## Wire notes

`uploadAttachment()` goes through the same `apiRequest()` as every other call
(`rawBody` + `contentType` options), so the bearer token, the
`X-Karucode-Client: web` CSRF header and the `ApiError` mapping are shared with
JSON requests. The composer sends `application/octet-stream` with the file bytes;
the server also accepts a real MIME type via the additive `contentType` option if
a future feature needs it.

## Automated suites

```bash
cd web
npx tsc --noEmit
npx vitest run src/components/Composer.test.tsx src/lib/mentions.test.ts src/lib/attachments.test.ts
npx vitest run          # whole web suite
```