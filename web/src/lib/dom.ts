import { useLayoutEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';

const activeTraps: symbol[] = [];
let scrollLocks = 0;
let baseBodyOverflow = '';

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
    if (scrollLocks === 0) baseBodyOverflow = document.body.style.overflow;
    scrollLocks += 1;
    document.body.style.overflow = 'hidden';
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;

    const focusFirst = window.setTimeout(() => {
      if (!ref.current) return;
      const preferred = ref.current.querySelector<HTMLElement>('[data-autofocus]');
      const first = preferred ?? focusableWithin(ref.current)[0];
      if (first) first.focus();
      else {
        ref.current.tabIndex = -1;
        ref.current.focus();
      }
    }, 0);

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
      window.clearTimeout(focusFirst);
      document.removeEventListener('keydown', onKeyDown, true);
      const index = activeTraps.lastIndexOf(trapId);
      if (index !== -1) activeTraps.splice(index, 1);
      scrollLocks = Math.max(0, scrollLocks - 1);
      if (scrollLocks === 0) document.body.style.overflow = baseBodyOverflow;
      if (previous && previous.isConnected) previous.focus();
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