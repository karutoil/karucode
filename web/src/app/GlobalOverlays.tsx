import type { JSX } from 'preact';
import { Suspense } from 'preact/compat';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { ToastRegion } from '../components/ToastRegion';
import { lazyNamed } from '../lib/lazyView';
import * as actions from '../state/actions';
import * as store from '../state/store';

/**
 * Every modal surface in the app, in one place.
 *
 * Each of these is a whole screen's worth of code - Settings is a form per
 * config section, the Theme studio is a colour engine, the file viewer carries
 * the markdown pipeline - and every one of them is closed on a normal page
 * load. They are therefore split into their own chunks AND mounted only while
 * their signal says they are open: the chunk is fetched the first time the
 * reader asks for the surface, and the surface is not even rendered while it is
 * closed.
 *
 * Mounting only while open is safe because none of these components owns a
 * global keybinding or a background read. `Mod+K`, `Mod+Shift+L`, Escape and
 * the "a dialog is open, ignore shortcuts" guard all live in
 * `app/useGlobalKeybindings.ts` and read the same signals this file reads, and
 * every overlay's own effects (the focus trap, the config read, the rewind
 * preview) are already gated on `open` inside the component. Their focus trap
 * restores focus from its unmount cleanup, which is what a close was doing
 * anyway.
 *
 * `ToastRegion` stays eager and outside every boundary: see below.
 */
const SettingsSheetLazy = lazyNamed(() => import('../components/SettingsSheet'), 'SettingsSheet');
const CommandPaletteLazy = lazyNamed(
  () => import('../components/CommandPalette'),
  'CommandPalette',
);
const ThemeStudioLazy = lazyNamed(() => import('../components/ThemeStudio'), 'ThemeStudio');
const NotificationCenterLazy = lazyNamed(
  () => import('../components/NotificationCenter'),
  'NotificationCenter',
);
const RewindDialogLazy = lazyNamed(() => import('../components/RewindDialog'), 'RewindDialog');
const FileViewerLazy = lazyNamed(() => import('../components/FileViewer'), 'FileViewer');

export function GlobalOverlays(): JSX.Element {
  return (
    <>
      {store.mobileSheet.value !== null ? (
        <button
          type="button"
          class="sheet-backdrop"
          aria-label="Close panel"
          onClick={actions.closeSheets}
        />
      ) : null}

      {store.settingsOpen.value ? (
        <ErrorBoundary scope="Settings">
          <Suspense fallback={null}>
            <SettingsSheetLazy />
          </Suspense>
        </ErrorBoundary>
      ) : null}
      {store.paletteOpen.value ? (
        <ErrorBoundary scope="The command palette">
          <Suspense fallback={null}>
            <CommandPaletteLazy />
          </Suspense>
        </ErrorBoundary>
      ) : null}
      {store.themeStudioOpen.value ? (
        <ErrorBoundary scope="The Theme studio">
          <Suspense fallback={null}>
            <ThemeStudioLazy />
          </Suspense>
        </ErrorBoundary>
      ) : null}
      {store.notificationCenterOpen.value ? (
        <ErrorBoundary scope="Notifications">
          <Suspense fallback={null}>
            <NotificationCenterLazy />
          </Suspense>
        </ErrorBoundary>
      ) : null}
      {store.rewindTarget.value !== null ? (
        <ErrorBoundary scope="The rewind dialog">
          <Suspense fallback={null}>
            <RewindDialogLazy />
          </Suspense>
        </ErrorBoundary>
      ) : null}
      {store.fileViewerPath.value !== null ? (
        <ErrorBoundary scope="The file viewer">
          <Suspense fallback={null}>
            <FileViewerLazy />
          </Suspense>
        </ErrorBoundary>
      ) : null}
      {/* The toast region stays OUTSIDE every boundary on purpose: it is how a
        * crash gets reported to the user ("could not save", "the stream died"),
        * so replacing it with a crash card would hide the very messages it exists
        * to deliver. It renders nothing but text and has no dependencies, so it
        * has nothing to throw about. It also stays eager: a toast for something
        * that happened while a chunk was still loading must not wait for it. */}
      <ToastRegion />
    </>
  );
}
