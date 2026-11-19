import type { JSX } from 'preact';
import { CommandPalette } from '../components/CommandPalette';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { FileViewer } from '../components/FileViewer';
import { NotificationCenter } from '../components/NotificationCenter';
import { RewindDialog } from '../components/RewindDialog';
import { SettingsSheet } from '../components/SettingsSheet';
import { ThemeStudio } from '../components/ThemeStudio';
import { ToastRegion } from '../components/ToastRegion';
import * as actions from '../state/actions';
import * as store from '../state/store';

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

      <ErrorBoundary scope="Settings">
        <SettingsSheet />
      </ErrorBoundary>
      <ErrorBoundary scope="The command palette">
        <CommandPalette />
      </ErrorBoundary>
      <ErrorBoundary scope="The Theme studio">
        <ThemeStudio />
      </ErrorBoundary>
      <ErrorBoundary scope="Notifications">
        <NotificationCenter />
      </ErrorBoundary>
      <ErrorBoundary scope="The rewind dialog">
        <RewindDialog />
      </ErrorBoundary>
      <ErrorBoundary scope="The file viewer">
        <FileViewer />
      </ErrorBoundary>
      {/* The toast region stays OUTSIDE every boundary on purpose: it is how a
        * crash gets reported to the user ("could not save", "the stream died"),
        * so replacing it with a crash card would hide the very messages it exists
        * to deliver. It renders nothing but text and has no dependencies, so it
        * has nothing to throw about. */}
      <ToastRegion />
    </>
  );
}
