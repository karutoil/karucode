import { useEffect } from 'preact/hooks';
import { matchUiKeybinding } from '../components/uiContributions';
import { isImeComposing } from '../lib/keyboard';
import { isApplePlatform } from '../lib/platform';
import * as actions from '../state/actions';
import { controlPlaneOpen } from '../state/controlPlane';
import * as railPeek from '../state/railPeek';
import * as store from '../state/store';
import * as workbench from '../state/workbench';
import { caretInEditable, isDialogOpen } from './helpers';

export function useGlobalKeybindings(): void {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (store.boot.value !== 'ready') return;
      const mod = event.metaKey || event.ctrlKey;
      const key = event.key.toLowerCase();

      // The caret's own keys. On Apple platforms the app's chords are ⌘-based, so
      // a bare Ctrl+letter inside a field is the field's emacs binding - Ctrl+B
      // cursor-left, Ctrl+K kill-to-end-of-line, Ctrl+J next line - and taking it
      // would move the caret *and* run a command. A ⌘ chord, or any Ctrl chord on
      // Windows/Linux where Ctrl *is* the app modifier, stays the app's even while
      // typing: Ctrl+K from the composer opening the palette is the only reason
      // anyone learns that chord. Nothing below runs `preventDefault()` before
      // this point, so an ignored chord reaches the field and the browser intact.
      if (event.ctrlKey && !event.metaKey && isApplePlatform() && caretInEditable(event)) return;

      // The one chord that is never a field's business on any platform: `Mod+N`
      // starts a new conversation, so it must not fire while the reader is
      // mid-sentence in the composer or an inline editor.
      if (mod && key === 'n' && caretInEditable(event)) return;

      // The Theme Studio used to sit on `Mod+Shift+T`, which every browser owns
      // for "reopen the closed tab" and does not reliably hand to the page. The
      // browser keeps that chord; the studio moved to `Mod+Shift+L`. It is a
      // *Shift* chord, so the Apple-only Ctrl guard above does not cover it, and
      // both engines treat it as a text chord: it does not fire from a field.
      if (mod && event.shiftKey && key === 'l') {
        if (caretInEditable(event)) return;
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
      // Escape belongs to whatever owns the caret first. The Context drawer is a
      // dialog on a phone with a focus trap of its own, and that trap closes it;
      // this branch is the key that still reaches the window - one a field inside
      // the drawer handled itself (a rename, a filter) or one that cancels an IME
      // candidate list. Neither may take the drawer away mid-word.
      if (
        event.key === 'Escape' &&
        workbench.contextOpen.value &&
        !event.defaultPrevented &&
        !caretInEditable(event) &&
        !isImeComposing(event)
      ) {
        event.preventDefault();
        workbench.closeContext();
        return;
      }
      // The phone's sheets are dialogs too (they are in `isDialogOpen()`), and their
      // focus trap closes them from the capture phase. This covers the press that
      // reaches the window instead - no trap in the path - and, like the trap,
      // leaves Escape to a field, to a control marked `data-escape-local`, and to an
      // IME that is using it to cancel a candidate.
      if (
        event.key === 'Escape' &&
        store.mobileSheet.value !== null &&
        !event.defaultPrevented &&
        !isImeComposing(event)
      ) {
        const target = event.target;
        if (
          caretInEditable(event) ||
          (target instanceof Element && target.closest('[data-escape-local]') !== null)
        ) {
          return;
        }
        actions.closeSheets();
        return;
      }
      if (isDialogOpen()) return;

      // The collapsed rail's fly-out is not a dialog and has no focus trap, so it
      // closes itself here — before the Escape that stops a run, which would
      // otherwise cancel a run when all the reader wanted was to dismiss the list.
      if (event.key === 'Escape' && railPeek.peekProjectId.value !== null) {
        railPeek.closePeek();
        return;
      }

      // `Mod+B` is the bookmarks bar (and, in Gecko, the bookmarks sidebar) in
      // every browser, so the rail owns `Mod+\` instead. A peek is a stand-in for
      // the rail: collapsing or restoring it is over.
      if (mod && key === '\\') {
        event.preventDefault();
        railPeek.closePeek();
        actions.toggleRail();
        return;
      }
      // `Mod+J` is Downloads in Gecko and cannot be counted on to reach the page;
      // the Context drawer answers to `Mod+Shift+X`. When the control plane owns
      // that drawer the chord belongs to the reader: do not swallow it.
      if (mod && event.shiftKey && key === 'x') {
        if (caretInEditable(event) || controlPlaneOpen.value) return;
        event.preventDefault();
        workbench.toggleContext();
        return;
      }
      if (mod && key === 'n') {
        event.preventDefault();
        actions.startDraftSession();
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
        // A popped-out Results panel is an overlay, not a layout column: Escape
        // dismisses it before it can be mistaken for "cancel the run".
        if (
          !store.narrowViewport.value &&
          store.inspectorMode.value === 'popout' &&
          !store.inspectorCollapsed.value
        ) {
          actions.toggleInspector();
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

  /**
   * Shortcuts plugins contributed.
   *
   * A separate listener from the app's own, on purpose: the two sets cannot
   * collide (a plugin binding must carry a modifier, and the host and the client
   * both refuse the app's own chords), so neither has to know about the other, and
   * a plugin that vanishes mid-session simply has no bindings in the next read.
   *
   * Two guards matter. No binding fires while a dialog is open — a shortcut that
   * acts behind a modal is a bug. And none fires while the caret is in a field: a
   * chord with a modifier is still a text-editing key there (Ctrl+A, Ctrl+Z), and
   * a plugin must not make typing surprising.
   */
  useEffect(() => {
    const onPluginKeyDown = (event: KeyboardEvent): void => {
      if (store.boot.value !== 'ready') return;
      if (event.defaultPrevented || isDialogOpen()) return;
      if (caretInEditable(event)) return;
      const binding = matchUiKeybinding(store.pluginUi.value.keybindings, event);
      if (!binding) return;
      event.preventDefault();
      void actions.runPluginContributions(binding);
    };
    window.addEventListener('keydown', onPluginKeyDown);
    return () => window.removeEventListener('keydown', onPluginKeyDown);
  }, []);
}
