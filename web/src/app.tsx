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
import { ToastRegion } from './components/ToastRegion';

function isDialogOpen(): boolean {
  return store.settingsOpen.value || store.paletteOpen.value || store.fileViewerPath.value !== null;
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

      if (mod && key === 'k') {
        event.preventDefault();
        actions.togglePalette();
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
      <FileViewer />
      <ToastRegion />
    </div>
  );
}