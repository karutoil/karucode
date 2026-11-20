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

/**
 * Is a modal surface in front of the workbench?
 *
 * Every signal here renders a `role="dialog"` (or a `Dialog`/`Overlay` shell)
 * that traps focus, so a shortcut or a plugin chord that fired behind one would
 * act on a surface the reader cannot reach. The phone's sessions and Results
 * sheets count: they are `aria-modal="true"` at 92vw, and the chat under them is
 * as unreachable as the chat behind Settings. The Context drawer is a dialog too,
 * but only below the breakpoint — docked, it is a column and the workspace keeps
 * its shortcuts.
 */
export function isDialogOpen(): boolean {
  return (
    store.settingsOpen.value ||
    store.paletteOpen.value ||
    store.themeStudioOpen.value ||
    store.notificationCenterOpen.value ||
    store.rewindTarget.value !== null ||
    store.fileViewerPath.value !== null ||
    store.mobileSheet.value !== null ||
    (store.narrowViewport.value && workbench.contextOpen.value)
  );
}
