import * as store from '../state/store';
import * as workbench from '../state/workbench';

/**
 * Is the caret somewhere a modifier chord still belongs to the field?
 *
 * A modifier does not make a shortcut safe. With `mod = metaKey || ctrlKey`, a
 * macOS reader types Ctrl+B (cursor-left), Ctrl+K (kill-to-end-of-line) and
 * Ctrl+J (newline in some editors) *as text-editing keys*, and Windows/Linux
 * readers have their own chords there. The shell's own shortcuts and the
 * plugin-contributed ones ask the same question, so they must ask it in one
 * place rather than drifting apart.
 */
export function caretInEditable(event: Event): boolean {
  const target = event.target;
  return (
    target instanceof HTMLElement &&
    target.closest('input, textarea, select, [contenteditable="true"]') !== null
  );
}

export function isDialogOpen(): boolean {
  return (
    store.settingsOpen.value ||
    store.paletteOpen.value ||
    store.themeStudioOpen.value ||
    store.notificationCenterOpen.value ||
    store.rewindTarget.value !== null ||
    store.fileViewerPath.value !== null ||
    (store.narrowViewport.value && workbench.contextOpen.value)
  );
}
