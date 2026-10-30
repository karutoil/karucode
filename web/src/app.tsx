// @kc-doc: web-ui
// @kc-title: Use the browser workspace
// @kc-summary: Panels, keyboard shortcuts and the dev loop for the Preact frontend that ships inside the binary.
// @kc-tags: ui, web, frontend, panels, shortcuts, preact
// @kc-applies-to: ui:ProjectRail, route:GET /api/projects/:id/favicon, ui:SessionsRail, ui:ChatView, ui:Composer, ui:Inspector, ui:SettingsSheet, ui:CommandPalette, ui:FileViewer, ui:FilePreview, ui:plugin-panel, route:GET /api/workspace/tree, route:PUT /api/workspace/file, route:GET /api/workspace/raw, route:POST /api/workspace/upload, route:GET /api/plugins/:name/panels/:panel, config:plugin_panels, file:web/src/components/pluginPanels.ts, file:web/src/components/PluginPanelView.tsx, file:web/src/lib/dockHeight.ts, fn:registerPluginPanelRenderer, event:tool_call_finished, tool:read_file, config:model.supports_images, cmd:npm run dev, cmd:npm run build, file:web/dist, env:KARUCODE_WEB_DIR
//
// # Layout
//
// * **Project rail** (left, 64 px): projects drawn as the project's own
//   `favicon.ico` when it ships one and as initials when it does not, a **Theme
//   studio** button, the **command palette** button and a settings shortcut.
//   Collapses to an icon column with `Ctrl/Cmd+B` on the session rail.
// * **Sessions rail**: the current project, a session search (`/`), sessions
//   grouped by last activity, newest first, with running indicators; create one
//   with `Ctrl/Cmd+N`.
// * **Chat**: streaming Markdown, tool cards whose footer carries the duration
//   and a **Show output** disclosure, command output, diff cards with a review
//   footer, approval cards, thinking blocks, and the composer with a model
//   switcher, @-file mentions and local-file attachments. An empty session
//   offers three starter prompts that put a whole instruction in the composer.
//   A tool card whose result carried images (the agent reading a screenshot with
//   a vision model) shows them as thumbnails above the output - visible without
//   expanding the card - and clicking one opens a lightbox with the full image.
//   The bytes come from `tool_call_finished.images` live and from the stored
//   `messages.images` on reload, so a preview never disappears after a refresh.
// * **Composer dock**: the composer *floats* over the bottom of the transcript
//   instead of taking a band of its own, so the conversation keeps the full
//   height of the pane. It starts on one line and grows with the draft. While you
//   are scrolled into history it turns see-through - 42% of the input surface,
//   plus a 3px backdrop blur - so the messages behind it stay readable, and the
//   status footnote slides away; clicking into the box (or sending) restores the
//   solid surface and the footnote. The gutter either side of the box is not a
//   hit target, so a wheel or drag there still scrolls the transcript. The
//   transcript reserves the dock's measured height, so nothing hides behind it at
//   rest, and **Jump to latest** sits above it rather than under it.
// * **Workspace tabs** (above the transcript): *Conversation* and *Changes*.
//   *Changes* reuses the inspector's git diff list; the composer only shows on
//   *Conversation*.
// * **Inspector** (right): tabs for agents, changes (git diff), commands and
//   workspace files, plus one tab per **plugin panel** (see below). Click a file
//   to open the viewer.
// * **File viewer** (modal): Markdown and HTML open on a rendered preview (with a
//   *Preview*/*Source* toggle), images and PDFs render from
//   `GET /api/workspace/raw`, and every complete text file can be edited and
//   saved. The preview follows your edits, so *Preview* is a live preview of the
//   draft. `Ctrl/Cmd+S` saves; a save whose `expected_size` no longer matches the
//   file offers **Overwrite anyway** rather than clobbering the agent's work;
//   closing with unsaved changes asks first. Binary and server-truncated files
//   cannot be edited (a truncated save would discard everything past the read
//   limit) and the *Edit* button's tooltip says which case applies.
// * **Theme studio** (right drawer, `Ctrl/Cmd+Shift+T`): built-in themes,
//   per-token colours, density and custom CSS, persisted in the browser. The
//   `themes` topic covers the storage key and the shareable export format.
// * **Settings sheet** (modal): **Appearance**, a shortcut into the Theme studio,
//   then provider profiles and their model libraries, generation, approvals,
//   limits, subagents, plugin/MCP diagnostics and the security note.
// * **Command palette** (`Ctrl/Cmd+K`): sessions, projects, actions.
//
// # Composer
//
// The box floats over the transcript as a *dock* instead of occupying its own
// row (see **Composer dock** above). What that means in practice:
//
// * it is translucent while you read history, so the messages behind it stay
//   visible; clicking into it, or scrolling back to the newest message, brings
//   the solid surface back;
// * it starts one line tall and grows with the draft up to 150px, then scrolls;
// * the transcript reserves the dock's measured height
//   (`--composer-h`, published by `lib/dockHeight.ts`), so the last message is
//   never hidden under it.
//
// Attach context three ways, all of which end up in the message the model reads:
//
// * type `@` to search the workspace index (`GET /api/workspace/tree?depth=6`;
//   `node_modules`, `.git`, `target`, `dist` and similar noise directories are
//   filtered out). Up/Down move, Enter or Tab inserts, Escape dismisses; a path
//   with spaces is inserted quoted (`@"docs/my plan.md"`);
// * press the `+` button to pick files;
// * drag files onto the composer, or paste them — a dashed overlay marks the drop
//   target.
//
// Files up to 64 KiB that look like text are inlined into the message as a fenced
// block (the head of the file is sniffed for NUL bytes). Everything else (binary,
// larger than 64 KiB) is uploaded with `POST /api/workspace/upload` and referenced
// by its workspace path, `.karucode/attachments/<session>/<name>`, which the agent
// can open with its file tools; names are never overwritten. Files above the 8 MiB
// upload ceiling are refused before any request is made. Sending is blocked while
// a chip is still reading or uploading, and a failed chip says why.
//
// # Plugin panels
//
// A plugin can add a tab to the inspector without shipping any UI. `GET
// /api/config` lists the panels ready plugins declare (`plugin_panels`), and the
// inspector appends one tab per panel after the built-in four. Opening a tab
// reads `GET /api/plugins/:name/panels/:panel` scoped by the current project and
// session.
//
// The payload is **untrusted JSON** and is drawn one of three ways:
//
// 1. by a renderer registered for that panel — a first-party component in the
//    app's own bundle, never plugin code;
// 2. as read-only JSON, when no renderer is registered or the renderer declines
//    the payload;
// 3. as a readable error, when the read failed, with a **Try again** button.
//
// To give a plugin a nicer view, register a renderer at startup:
//
// ```ts
// import { registerPluginPanelRenderer } from './components/pluginPanels';
//
// registerPluginPanelRenderer('plugin__kc-todo__todo', {
//   matches: (data) => Array.isArray((data as { items?: unknown })?.items),
//   render: ({ data, refresh }) => <TodoTab data={data} onRefresh={refresh} />,
// });
// ```
//
// `matches` gets `unknown`; validate before reading a field. The registry
// prefers a renderer registered for the hosted id (`plugin__<plugin>__<id>`) and
// falls back to the bare id, so one plugin's panel can be styled without
// affecting another's panel of the same name.
//
// Panel data must never reach `dangerouslySetInnerHTML`, must never be used to
// build a URL, and must never be evaluated: that is the whole point of shipping
// data instead of markup, and it is why the page keeps `frame-ancestors 'none'`
// and a CSP with no `unsafe-eval`. The generic JSON view follows the same rule —
// a string that looks like markup is shown as text.
//
// A browser cannot reach a plugin any other way: there is no plugin route
// mounting, no asset serving and no script execution. A panel is a tab whose
// *contents* the app draws from the plugin's data.
//
// # Keyboard shortcuts
//
// | Keys | Action |
// | --- | --- |
// | `Ctrl/Cmd+K` | command palette |
// | `Ctrl/Cmd+N` | new session |
// | `Ctrl/Cmd+B` | toggle the sessions rail |
// | `Ctrl/Cmd+J` | toggle the inspector |
// | `Ctrl/Cmd+Shift+T` | open or close the Theme studio |
// | `/` | focus the session search |
// | `Up`/`Down` | move through the `@`-mention suggestions |
// | `Enter`/`Tab` | insert the highlighted mention |
// | `Escape` | stop the running turn (or close the top-most sheet/menu) |
//
// # Mobile
//
// Below 1024 px the rails become sheets driven by the mobile toolbar. `Escape`
// closes the open sheet first and stops the run only when nothing else is open.
//
// # Development loop
//
// ```bash
// cd web
// npm ci
// npm run dev        # Vite on :5173, proxies /api to :8787
// npm test           # vitest reducer/component tests
// npm run typecheck
// npm run build      # emits web/dist, embedded on the next cargo build
// ```
//
// Run the backend separately (`cargo run -p kc-server`) while `npm run dev` proxies
// to it. To serve your build without rebuilding the binary, start the server with
// `KARUCODE_WEB_DIR=web/dist`.
//
// # State and events
//
// The UI is Preact + signals. It bootstraps from `/api/health` and `/api/config`,
// then follows one SSE stream (`/api/events`) and reduces events into the store
// (`src/state/eventReducer.ts`). There is no websocket and no polling loop except
// a 20 s health check.
//
// The stream watches itself so live updates never need a page reload: 45 s without
// a byte (keepalives included) means the connection is dead and it reconnects with
// backoff; a jump in `seq` or a `stream_lagged` marker means frames were lost, so
// the open session is refetched automatically. The refetch always runs - that is
// how a stuck "running" flag clears when the terminal event was the lost one - but
// it only *replaces* the transcript when the server says no run is in flight, so an
// answer that is still streaming is never deleted.
//
// # Gotchas
//
// * The UI embedded in a release binary is a compile-time snapshot. Rebuilding the
//   frontend alone changes nothing until `cargo build` runs again, unless you set
//   `KARUCODE_WEB_DIR`.
// * The project rail's icon comes from `GET /api/projects/:id/favicon`, fetched
//   with the API client rather than drawn from an `<img src>`: an `<img>` cannot
//   send `Authorization`, and only `/api/events` accepts `?token=`. The bytes
//   become an object URL per project, so adding the same project twice does not
//   duplicate a request, and a project with no favicon answers `204` once per
//   page load and then draws initials. That `blob:` URL is why the page CSP
//   allows `blob:` in `img-src`; remote images stay blocked. There is no manual
//   "choose an icon" setting.
// * The API is same-origin: the dev server proxies `/api`, so run the backend
//   alongside it instead of pointing the browser at two origins.
// * `Ctrl/Cmd+N` is intercepted only while no dialog is open; inside the composer
//   the browser default is left alone where it matters.
//
// Full reference: `README.md` (Development); panel and shortcut details are in
// the generated `docs/internal/web-ui.md`.

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import * as actions from './state/actions';
import * as store from './state/store';
import { AuthScreen } from './components/AuthScreen';
import { BootErrorScreen, BootSkeleton } from './components/BootScreens';
import { ChatView } from './components/ChatView';
import { CommandPalette } from './components/CommandPalette';
import { FileViewer } from './components/FileViewer';
import { Inspector } from './components/Inspector';
import { MobileToolbar } from './components/MobileToolbar';
import { ProjectRail } from './components/ProjectRail';
import { SessionsRail } from './components/SessionsRail';
import { SettingsSheet } from './components/SettingsSheet';
import { ThemeStudio } from './components/ThemeStudio';
import { ToastRegion } from './components/ToastRegion';

function isDialogOpen(): boolean {
  return (
    store.settingsOpen.value ||
    store.paletteOpen.value ||
    store.themeStudioOpen.value ||
    store.fileViewerPath.value !== null
  );
}

export function App(): JSX.Element {
  useEffect(() => {
    const unregisterAuth = actions.registerAuthHandler();
    void actions.bootstrap();

    const clockTimer = window.setInterval(() => {
      store.clock.value = Date.now();
    }, 20_000);
    const healthTimer = window.setInterval(() => {
      void actions.checkHealth();
    }, 20_000);

    const media =
      typeof window.matchMedia === 'function' ? window.matchMedia('(max-width: 1023px)') : null;
    const updateViewport = (): void => {
      store.narrowViewport.value = media ? media.matches : false;
      if (media && !media.matches) store.mobileSheet.value = null;
    };
    updateViewport();
    if (media) {
      if (typeof media.addEventListener === 'function') media.addEventListener('change', updateViewport);
      else if (typeof media.addListener === 'function') media.addListener(updateViewport);
    }

    return () => {
      unregisterAuth();
      window.clearInterval(clockTimer);
      window.clearInterval(healthTimer);
      if (media) {
        if (typeof media.removeEventListener === 'function') media.removeEventListener('change', updateViewport);
        else if (typeof media.removeListener === 'function') media.removeListener(updateViewport);
      }
      actions.stopStream();
    };
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (store.boot.value !== 'ready') return;
      const mod = event.metaKey || event.ctrlKey;
      const key = event.key.toLowerCase();

      if (mod && event.shiftKey && key === 't') {
        event.preventDefault();
        actions.toggleThemeStudio();
        return;
      }
      if (mod && key === 'k') {
        event.preventDefault();
        actions.togglePalette();
        return;
      }
      // The Theme Studio owns no key handler of its own, so Escape must close it
      // before the generic "a dialog is open, ignore shortcuts" guard below.
      if (event.key === 'Escape' && store.themeStudioOpen.value) {
        actions.closeThemeStudio();
        return;
      }
      if (isDialogOpen()) return;

      if (mod && key === 'b') {
        event.preventDefault();
        actions.toggleRail();
        return;
      }
      if (mod && key === 'j') {
        event.preventDefault();
        actions.toggleInspector();
        return;
      }
      if (mod && key === 'n') {
        // Unlike the other shortcuts this one creates a session, so it must not
        // fire while the user is typing: `Ctrl+N` is a text-editing key in the
        // composer and in every inline editor.
        const target = event.target;
        if (
          target instanceof HTMLElement &&
          target.closest('input, textarea, select, [contenteditable="true"]')
        ) {
          return;
        }
        event.preventDefault();
        void actions.createSession();
        return;
      }
      if (event.key === 'Escape') {
        const target = event.target;
        // The composer handles Escape itself; inline editors and open menus
        // consume it so closing them does not also cancel the run.
        if (
          target instanceof HTMLElement &&
          target.closest('input, textarea, select, [contenteditable="true"], [role="menu"]')
        ) {
          return;
        }
        if (store.mobileSheet.value !== null) {
          actions.closeSheets();
          return;
        }
        if (store.chat.value.running && !store.stopping.value) {
          void actions.stopRun();
        }
      }
    };

    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  const session = store.activeSession.value;
  useEffect(() => {
    document.title = session?.title ? `${session.title} — karucode` : 'karucode';
  }, [session?.title]);

  if (store.boot.value === 'loading') return <BootSkeleton />;
  if (store.boot.value === 'auth') return <AuthScreen />;
  if (store.boot.value === 'error') return <BootErrorScreen />;

  return (
    <div class="app-root">
      <a class="skip-link" href="#chat-main">
        Skip to chat
      </a>
      <div
        class={`app${store.narrowViewport.value ? ' is-narrow' : ''}`}
        data-rail-collapsed={store.railCollapsed.value ? 'true' : 'false'}
        data-inspector-collapsed={store.inspectorCollapsed.value ? 'true' : 'false'}
      >
        <ProjectRail />
        <SessionsRail />
        <ChatView />
        <Inspector />
      </div>

      {store.mobileSheet.value !== null ? (
        <button
          type="button"
          class="sheet-backdrop"
          aria-label="Close panel"
          onClick={actions.closeSheets}
        />
      ) : null}

      <MobileToolbar />
      <SettingsSheet />
      <CommandPalette />
      <ThemeStudio />
      <FileViewer />
      <ToastRegion />
    </div>
  );
}