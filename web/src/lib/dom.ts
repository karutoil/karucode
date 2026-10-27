import { useEffect, useRef } from 'preact/hooks';
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
  '[contenteditable="true"]',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

export function focusableWithin(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (element) =>
      element.offsetParent !== null ||
      element === document.activeElement ||
      // jsdom does not compute layout, so keep elements that are merely hidden via CSS.
      !element.hasAttribute('hidden'),
  );
}

/**
 * Traps focus inside a dialog, closes on Escape, locks page scrolling and
 * restores focus to the previously active element on unmount. Dialogs are only
 * rendered while open, so `active` is an explicit escape hatch for tests.
 */
export function useFocusTrap(
  ref: RefObject<HTMLElement>,
  active: boolean,
  onEscape?: () => void,
): void {
  const escapeRef = useRef(onEscape);
  escapeRef.current = onEscape;

  useEffect(() => {
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