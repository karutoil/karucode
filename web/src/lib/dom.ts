import { useLayoutEffect, useRef } from 'preact/hooks';
import { isImeComposing } from './keyboard';
import type { RefObject } from 'preact';

const activeTraps: symbol[] = [];
let scrollLocks = 0;
let baseBodyOverflow = '';
let baseRootOverflow = '';
let baseBodyOverscroll = '';
let baseRootOverscroll = '';
/** Where the document stood when the first lock was taken. */
let lockedScrollY = 0;

/**
 * Where focus goes when the element that opened a surface no longer exists.
 *
 * A transcript row the window swapped out, a menu item a rerun removed — the
 * closer has nothing to hand focus back to, and `document.body` means the next
 * Tab restarts at the very top of the app. The shell's landmarks are the
 * structural answer. `tabindex="-1"` keeps a landmark out of the Tab order while
 * still letting it take focus, so the next Tab lands on its first control.
 *
 * `closing` is the surface being torn down. A landmark *inside* it would be
 * removed with it, and handing focus to something about to disappear puts it
 * right back on `body` — the bug this exists to fix. So the candidates a surface
 * nested in a landmark rules out are its descendants, not its ancestors: when
 * `Inspector` or `ContextDrawer` renders inside `<main id="chat-main">`, that
 * `<main>` is exactly the landmark to fall back to; it outlives the surface.
 */
const FOCUS_LANDMARK_SELECTOR = 'main, .workbench';

function lastResortFocusTarget(closing?: HTMLElement | null): HTMLElement | null {
  const candidates = document.querySelectorAll<HTMLElement>(FOCUS_LANDMARK_SELECTOR);
  for (const landmark of candidates) {
    if (!closing) return landmark;
    // The surface itself, or a landmark the surface contains: both die with it.
    if (landmark === closing || closing.contains(landmark)) continue;
    return landmark;
  }
  return null;
}

const FOCUSABLE_SELECTOR = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  'details > summary:first-of-type',
  '[contenteditable="true"]',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

/**
 * Controls that consume Escape themselves.
 *
 * The trap listens on `document` in the capture phase, so it sees the key before
 * the target does. Without this list the mobile sessions sheet would close as a
 * whole before its search field could clear, its rename input could cancel, or
 * its menus could dismiss. Only explicit `data-escape-local` opts in: a menu or
 * listbox role describes semantics, not proof that the component handles Escape.
 */
const LOCAL_ESCAPE_SELECTOR = '[data-escape-local]';

/**
 * Escape belongs to a control that opts in with `data-escape-local`.
 *
 * Deliberately not "any input": a *modal form* (rename a project, edit a
 * provider) is expected to close on Escape even from its own field, and deferring
 * for every caret broke exactly that. A surface where the field owns Escape - a
 * search box that clears, a filter, a menu - marks the control instead, and the
 * window-level handler defers to editable targets in its own branches
 * (`useGlobalKeybindings`).
 */

/**
 * Does this document have layout at all?
 *
 * jsdom reports every `offsetParent` as `null` and a 0×0 client box, because it
 * does not lay anything out. The previous visibility gate was
 * `offsetParent !== null`, which is false for *every* node there, so the fallback
 * had to keep everything and hidden content stayed tabbable. Detecting the
 * layout-less document lets the fallback be strict about the things jsdom *can*
 * answer (the cascade, `hidden`, `inert`, a closed `<details>`) without
 * demanding geometry that will never exist.
 */
function hasLayout(): boolean {
  if (typeof document === 'undefined') return false;
  return document.documentElement.clientWidth > 0 && document.documentElement.clientHeight > 0;
}

function computed(element: HTMLElement, property: string): string {
  if (typeof window === 'undefined' || typeof window.getComputedStyle !== 'function') return '';
  return window.getComputedStyle(element).getPropertyValue(property);
}

/** Is `element` inside the always-visible `<summary>` of a closed details? */
function isSummaryContent(details: HTMLElement, element: HTMLElement): boolean {
  const summary = Array.from(details.children).find((child) => child.tagName === 'SUMMARY');
  return summary !== undefined && summary.contains(element);
}

/**
 * Is this element actually visible to a user?
 *
 * Three layers, strongest first, so the answer means the same thing in a browser
 * and in a test:
 *
 * 1. `checkVisibility()` when the engine has it — it follows the real cascade
 *    (`display`, `visibility`, `content-visibility`) and is the one answer that
 *    accounts for every ancestor;
 * 2. semantics and the cascade — `hidden`, `inert` and a closed `<details>` hide
 *    a subtree without touching its `display`, and `display: none` on any
 *    ancestor (or `visibility: hidden` on the element, which a descendant may
 *    turn back on) removes the box;
 * 3. layout, only when the document has any: an element with no client rects is
 *    not painted.
 */
function isVisible(element: HTMLElement): boolean {
  const check = (
    element as HTMLElement & {
      checkVisibility?: (options?: Record<string, boolean>) => boolean;
    }
  ).checkVisibility;
  const engineAnswered = typeof check === 'function';
  if (engineAnswered) {
    // Both option spellings: the first pair is the original draft (Chrome 105),
    // the second is the standardised name. Unknown keys are ignored, so an engine
    // reads whichever generation it implements.
    const visible = check.call(element, {
      checkOpacity: false,
      checkVisibilityCSS: true,
      opacityProperty: false,
      visibilityProperty: true,
    });
    if (!visible) return false;
  } else {
    // No `checkVisibility` (jsdom): the cascade still exists, and `visibility`
    // is inherited but re-declarable per element, so read it on the element.
    const visibility = computed(element, 'visibility');
    if (visibility === 'hidden' || visibility === 'collapse') return false;
  }

  for (let node: HTMLElement | null = element; node; node = node.parentElement) {
    if (node.hasAttribute('hidden') || node.hasAttribute('inert')) return false;
    if (
      node !== element &&
      node.tagName === 'DETAILS' &&
      !(node as HTMLDetailsElement).open &&
      !isSummaryContent(node, element)
    ) {
      return false;
    }
    // `checkVisibility` already accounted for ancestor `display: none`; walking
    // it again would mean a style flush per ancestor on every Tab press.
    if (!engineAnswered && computed(node, 'display') === 'none') return false;
  }

  if (hasLayout() && typeof element.getClientRects === 'function') {
    if (element.getClientRects().length === 0) return false;
  }
  return true;
}

/**
 * The focusable descendants of `container` a user can actually reach.
 *
 * Hidden, inert and closed-`<details>` content is skipped so Tab cannot land
 * somewhere invisible; when nothing is left the trap keeps focus on the surface
 * itself instead of leaking to the page behind it.
 */
export function focusableWithin(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    // `tabindex="-1"` is programmatic focus only: a native button with it still
    // matches the `button` selector above, so the exclusion has to be explicit.
    (element) => element.getAttribute('tabindex') !== '-1' && isVisible(element),
  );
}

/**
 * Traps focus inside a dialog, closes on Escape, locks page scrolling and
 * restores focus to the previously active element on unmount. Dialogs are only
 * rendered while open, so `active` is an explicit escape hatch for tests.
 *
 * The listener and stack entry must be installed in the same commit as the
 * visible surface. A passive effect leaves a frame-sized gap in which trusted
 * Escape reaches the application shortcut handler, which ignores open dialogs,
 * and the key is lost.
 */
export function useFocusTrap(
  ref: RefObject<HTMLElement | null>,
  active: boolean,
  onEscape?: () => void,
): void {
  const escapeRef = useRef(onEscape);
  escapeRef.current = onEscape;

  useLayoutEffect(() => {
    if (!active) return;
    const container = ref.current;
    if (!container) return;
    const trapId = Symbol('focus-trap');
    activeTraps.push(trapId);
    if (scrollLocks === 0) {
      const body = document.body;
      const root = document.documentElement;
      baseBodyOverflow = body.style.overflow;
      baseRootOverflow = root.style.overflow;
      baseBodyOverscroll = body.style.overscrollBehavior;
      baseRootOverscroll = root.style.overscrollBehavior;
      // `overflow: hidden` on `body` is not a lock on iOS: Safari pans the visual
      // viewport anyway, and some mobile browsers keep the document parked and
      // reset the position on release. The scroll position is remembered so it can
      // be put back, and `overscroll-behavior: contain` stops a scroller *inside*
      // the dialog from handing its momentum to the page behind. `touch-action` is
      // deliberately left alone — it applies to the dialog's own scrollers too and
      // would stop them panning.
      lockedScrollY = typeof window.scrollY === 'number' ? window.scrollY : 0;
      body.style.overflow = 'hidden';
      root.style.overflow = 'hidden';
      body.style.overscrollBehavior = 'contain';
      root.style.overscrollBehavior = 'contain';
    }
    scrollLocks += 1;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;

    // Synchronously, in the same frame the portal is committed — not in a
    // `setTimeout(…, 0)`. iOS only raises the software keyboard for a `focus()`
    // that happens inside the user's own gesture, and a timer is a new task: the
    // command palette opened with a caret and no keyboard, and the reader had to
    // tap the field again (SH-10, OV-11). `useLayoutEffect` runs after the DOM is
    // in place and before paint, still inside the press that opened the surface,
    // so this is the earliest moment the field and the gesture are both real. The
    // trap's own semantics (first `[data-autofocus]`, else the first focusable,
    // else the surface itself) are unchanged.
    const focusFirst = (): void => {
      if (!ref.current) return;
      const preferred = ref.current.querySelector<HTMLElement>('[data-autofocus]');
      const first = preferred ?? focusableWithin(ref.current)[0];
      if (first) first.focus();
      else {
        ref.current.tabIndex = -1;
        ref.current.focus();
      }
    };
    focusFirst();

    const onKeyDown = (event: KeyboardEvent): void => {
      // Nested surfaces (a confirm dialog over a mobile sheet) form a stack;
      // only the topmost trap handles keys.
      if (activeTraps[activeTraps.length - 1] !== trapId) return;
      if (event.key === 'Escape') {
        // A control inside the surface may own Escape: the sessions sheet's
        // search field clears, a rename input cancels, a menu dismisses. Capture
        // runs before the target, so deferring is the only way the local handler
        // ever sees the key. It must also not stop propagation: the local
        // handler is the one that decides whether anything else closes.
        if (isImeComposing(event)) return;
        if (
          event.target instanceof Element &&
          event.target.closest(LOCAL_ESCAPE_SELECTOR) !== null
        ) {
          return;
        }
        event.stopPropagation();
        escapeRef.current?.();
        return;
      }
      if (event.key !== 'Tab') return;
      const dialog = ref.current;
      if (!dialog) return;
      const focusable = focusableWithin(dialog);
      if (focusable.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (!first || !last) return;
      const activeElement = document.activeElement;
      if (!dialog.contains(activeElement)) {
        event.preventDefault();
        (event.shiftKey ? last : first).focus();
      } else if (event.shiftKey && activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener('keydown', onKeyDown, true);
    return () => {
      document.removeEventListener('keydown', onKeyDown, true);
      const index = activeTraps.lastIndexOf(trapId);
      if (index !== -1) activeTraps.splice(index, 1);
      scrollLocks = Math.max(0, scrollLocks - 1);
      if (scrollLocks === 0) {
        const body = document.body;
        const root = document.documentElement;
        body.style.overflow = baseBodyOverflow;
        root.style.overflow = baseRootOverflow;
        body.style.overscrollBehavior = baseBodyOverscroll;
        root.style.overscrollBehavior = baseRootOverscroll;
        // Put the position back only when the browser reset it. A page that is
        // still where it was needs nothing, and a page the reader moved (a mobile
        // browser lets the page behind pan while `overflow: hidden` is in force)
        // must be left alone.
        const y = typeof window.scrollY === 'number' ? window.scrollY : 0;
        const wasReset = y === 0 && lockedScrollY !== 0;
        if (wasReset && typeof window.scrollTo === 'function') {
          window.scrollTo(0, lockedScrollY);
        }
        // This lock is over: a later one records its own position rather than
        // inheriting this one's and "restoring" the reader to where an older
        // surface happened to be opened.
        lockedScrollY = 0;
      }
      if (previous && previous.isConnected && previous !== document.body) previous.focus();
      else {
        // The opener is gone: a transcript row the window swapped out, a menu
        // item a rerun removed. Nothing focused means the next Tab restarts at
        // the top of the document, so focus a landmark instead. Anything already
        // focused *outside* this surface is a deliberate choice — the focus
        // still inside it is this surface's own default and dies with it.
        // `<body>` is not an opener: it is what "nothing was focused" looks like,
        // and handing focus back to it is the very thing being avoided.
        const active = document.activeElement;
        const keepsFocus =
          active instanceof HTMLElement &&
          active !== document.body &&
          active.isConnected &&
          !container.contains(active);
        // The restore above is only worth anything if focus does not scroll the
        // landmark it lands on into view: `focus()` pans by default, so the
        // position just put back would move again.
        if (!keepsFocus) {
          const landmark = lastResortFocusTarget(container);
          if (landmark) {
            // `tabindex="-1"` is left in place rather than removed afterwards.
            // In Blink, removing it while the element is focused hands focus back
            // to `body` - on the next task, on a timer, on a frame, every way
            // (measured); only keeping it keeps the focus. And on a landmark it is
            // exactly the right state: programmatically focusable, out of the Tab
            // order. `main#chat-main` already carries it, so this adds nothing
            // there and only gives the same property to a landmark that lacked it.
            if (!landmark.hasAttribute('tabindex')) landmark.tabIndex = -1;
            landmark.focus({ preventScroll: true });
          }
        }
      }
    };
  }, [active, ref]);
}

export function isNarrowViewport(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false;
  return window.matchMedia('(max-width: 1023px)').matches;
}

export function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false;
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}