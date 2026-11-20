import { useEffect } from 'preact/hooks';

/** The shape `useRef` hands back; spelled locally so this module needs no
 *  Preact type import. */
interface ElementRef {
  current: HTMLElement | null;
}

/**
 * Shell regions that must never be inerted even when they are a covered sibling.
 * `.sheet-backdrop` is how a phone closes a sheet at all, and `.app-root`'s
 * non-`.app` children are the teardown chrome: the backdrop, the toasts and the
 * overlay hosts that carry every dialog which can open *over* a sheet. Inerting
 * those would leave a modal with no way to dismiss it.
 *
 * Everything else behind a sheet is dead surface and is marked so — including
 * the conversation, which is the finding (A11Y-05).
 */
const SHELL_HOSTS = [
  '.sheet-backdrop',
  '.app-root > :not(.app):not(.skip-link)',
].join(', ');

/**
 * Mark everything a phone sheet covers as `inert` while it is open.
 *
 * The sheets are `role="dialog"` + `aria-modal="true"` but they are *siblings* of
 * the surface they cover, not overlays on top of it, so a screen reader and the
 * Tab order still reached the chat behind them and global shortcuts fired on a
 * conversation nobody could see (A11Y-05). `inert` is the DOM's own answer: the
 * subtree is skipped by focus, by the accessibility tree and by hit-testing.
 *
 * Walking up from the sheet to `<body>` and inerting every *sibling* along the
 * way is what makes this correct for both cases at once: a top-level sheet (the
 * sessions rail, a child of `.app`) and a nested one (the inspector, a child of
 * the chat column). Nothing the sheet needs stays covered, and no selector has
 * to be agreed on with the layout that happens to own the shell today.
 *
 * Cleanup removes only the attributes this hook added, so a region that was
 * already inert for its own reasons (the inspector under a context drawer) keeps
 * its own state.
 */
export function useCoveredInert(ref: ElementRef, active: boolean): void {
  useEffect(() => {
    const sheet = ref.current;
    if (!active || !sheet || typeof document === 'undefined') return undefined;

    const covered: HTMLElement[] = [];
    let node: HTMLElement | null = sheet;
    while (node && node !== document.body) {
      const parent: HTMLElement | null = node.parentElement;
      if (!parent) break;
      for (const sibling of Array.from(parent.children)) {
        if (sibling === node || !(sibling instanceof HTMLElement)) continue;
        if (sibling.hasAttribute('inert') || covered.includes(sibling)) continue;
        // The app's own overlay hosts stay live: the backdrop that closes a
        // sheet, the toasts and the dialogs that open *over* it live in sibling
        // regions, and inerting them would leave a modal with no way to dismiss
        // it.
        if (sibling.matches(SHELL_HOSTS)) continue;
        sibling.setAttribute('inert', '');
        covered.push(sibling);
      }
      node = parent;
    }

    return () => {
      for (const element of covered) element.removeAttribute('inert');
    };
  }, [ref, active]);
}
