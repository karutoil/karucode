// @kc-doc: web-ui
// @kc-title: Use the browser workspace
// @kc-summary: Panels, keyboard shortcuts and the dev loop for the Preact frontend that ships inside the binary.
// @kc-tags: ui, web, frontend, panels, shortcuts, preact
// @kc-applies-to: ui:ProjectRail, ui:SessionsRail, ui:ChatView, ui:Composer, ui:Inspector, ui:SettingsSheet, ui:CommandPalette, ui:FileViewer, route:GET /api/workspace/tree, route:POST /api/workspace/upload, cmd:npm run dev, cmd:npm run build, file:web/dist, env:KARUCODE_WEB_DIR
//
// # Layout
//
// * **Project rail** (left, 64 px): projects, a **Theme studio** button, the
//   **command palette** button and a settings shortcut. Collapses to an icon
//   column with `Ctrl/Cmd+B` on the session rail.
// * **Sessions rail**: the current project, a session search (`/`), sessions
//   grouped by last activity, newest first, with running indicators; create one
//   with `Ctrl/Cmd+N`.
// * **Chat**: streaming Markdown, tool cards whose footer carries the duration
//   and a **Show output** disclosure, command output, diff cards with a review
//   footer, approval cards, thinking blocks, and the composer with a model
//   switcher, @-file mentions and local-file attachments. An empty session
//   offers three starter prompts that put a whole instruction in the composer.
// * **Workspace tabs** (above the transcript): *Conversation* and *Changes*.
//   *Changes* reuses the inspector's git diff list; the composer only shows on
//   *Conversation*.
// * **Inspector** (right): tabs for agents, changes (git diff), commands and
//   workspace files. Click a file to open the read-only viewer.
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
// * The API is same-origin: the dev server proxies `/api`, so run the backend
//   alongside it instead of pointing the browser at two origins.
// * `Ctrl/Cmd+N` is intercepted only while no dialog is open; inside the composer
//   the browser default is left alone where it matters.
//
// Full reference: `README.md` (UI, Development).

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