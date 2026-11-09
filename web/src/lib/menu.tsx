import type { ComponentChildren, JSX } from 'preact';
import { useCallback, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { createPortal } from 'preact/compat';

/**
 * One accessible menu, for every surface that needs one.
 *
 * Before this file existed the product shipped five near-identical `role="menu"`
 * implementations - the workbench task menu, the control-plane row kebab, the
 * sessions rail row kebab, the project switcher and the project rail menu - each
 * with its own copy of the arrow-key walk, its own outside-click listener (two
 * different event types between them), its own Escape timing and its own focus
 * restore. They had already drifted: one filtered disabled items and the others
 * did not, one restored focus on a timer, one leaked focus to `<body>` when an
 * item opened a dialog, and none of them could keep a menu on screen.
 *
 * The contract implemented here, once:
 *
 *   - opening focuses the first enabled item, and that item is the only one in
 *     the Tab order (roving tabindex);
 *   - ArrowDown/ArrowUp wrap, Home/End jump, over enabled items only;
 *   - Escape closes and returns focus to the trigger, and owns that Escape
 *     (`data-escape-local`) so a dialog layer's focus trap defers to the menu
 *     instead of closing the sheet underneath it;
 *   - Tab closes and returns focus to the trigger rather than leaking focus into
 *     a menu that is about to disappear;
 *   - a pointerdown outside the menu and its trigger closes it, restoring focus
 *     only when the menu had it, so clicking elsewhere does not yank focus back
 *     to a kebab;
 *   - `invoke()` focuses the trigger *before* closing and running the action, so
 *     a dialog opened from a menu item always records a connected return target;
 *   - a pointer-anchored menu (`openAtPointer`, i.e. right-click) renders through
 *     a portal, clamped to the viewport, because a row near the bottom of a long
 *     rail would otherwise open its menu off-screen; it closes on scroll and
 *     resize because its anchor is a snapshot of where the pointer was.
 *
 * Two placements, one behaviour. A trigger-anchored menu renders inline where the
 * caller puts it and is positioned by the shared CSS (`.session-menu`,
 * `.rail-menu`, …), which is what the style contracts pin. A pointer-anchored
 * menu renders through a portal on the menu layer: above the app chrome and
 * below the dialog layer (§5B of `docs/internal/DESIGN-SYSTEM.md`).
 */

export interface MenuAction {
  /** The item's visible label and its accessible name. */
  label: string;
  /** An icon rendered before the label; decorative, so the label carries the name. */
  icon?: ComponentChildren;
  onSelect: () => void;
  disabled?: boolean;
  /** Destructive actions get the danger ink (`.menu-item-danger`). */
  danger?: boolean;
  /** Advertised keyboard shortcut, e.g. `'Ctrl+K'`; informational only. */
  keyshortcuts?: string;
  title?: string;
}

interface Pointer {
  x: number;
  y: number;
}

export interface MenuController {
  open: boolean;
  /** The element focus returns to on close: the kebab, or the right-clicked row. */
  triggerRef: { current: HTMLElement | null };
  menuRef: { current: HTMLDivElement | null };
  /** Pointer coordinates when the menu was opened by right-click, else null. */
  pointer: Pointer | null;
  /** Kebab/trigger click. */
  toggle: () => void;
  /** Right-click: anchor at the pointer and suppress the native menu. */
  openAtPointer: (event: MouseEvent) => void;
  /** Anchor at explicit coordinates (a kebab's rect) with the same clamping. */
  openAt: (x: number, y: number, trigger?: HTMLElement | null) => void;
  close: (restoreFocus?: boolean) => void;
  /** Focus the trigger, close, then run: the dialog-safe way to run an item. */
  invoke: (action: () => void) => void;
  /** Spread on the element that opens the menu (the kebab button). */
  triggerProps: {
    ref: (el: HTMLElement | null) => void;
    'aria-haspopup': 'menu';
    'aria-expanded': boolean;
    onClick: () => void;
  };
}

const ITEMS = '[role="menuitem"]:not(:disabled)';

/** One item in the Tab order: the focused one, never two, never zero while open.
 *  Every `[role="menuitem"]` is set, disabled ones included: a disabled button
 *  keeps the browser's default tab index unless told otherwise. */
function setRoving(root: HTMLElement | null, focused: HTMLButtonElement): void {
  for (const item of Array.from(root?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [])) {
    item.tabIndex = item === focused ? 0 : -1;
  }
}

export function useMenu(): MenuController {
  const [open, setOpen] = useState(false);
  const [pointer, setPointer] = useState<Pointer | null>(null);
  const triggerRef = useRef<HTMLElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);

  const close = useCallback((restoreFocus = false): void => {
    setOpen(false);
    setPointer(null);
    if (restoreFocus && triggerRef.current?.isConnected) triggerRef.current.focus();
  }, []);

  const toggle = useCallback((): void => {
    setPointer(null);
    setOpen((current) => !current);
  }, []);

  const openAt = useCallback((x: number, y: number, trigger?: HTMLElement | null): void => {
    if (trigger !== undefined) triggerRef.current = trigger;
    setPointer({ x, y });
    setOpen(true);
  }, []);

  const openAtPointer = useCallback((event: MouseEvent): void => {
    // Suppressing the native menu is the whole point of offering our own; which
    // rows qualify is the caller's decision, made by wiring this handler at all.
    event.preventDefault();
    openAt(event.clientX, event.clientY,
      event.currentTarget instanceof HTMLElement ? event.currentTarget : null);
  }, [openAt]);

  const invoke = useCallback((action: () => void): void => {
    // A focused menu item that unmounts cannot receive the focus a dialog's trap
    // gives back, so the trigger takes focus first, while it is still connected.
    if (triggerRef.current?.isConnected) triggerRef.current.focus();
    setOpen(false);
    setPointer(null);
    action();
  }, []);

  // Opening focuses the first enabled item: a menu the keyboard cannot find is
  // not a menu, and the roving tab index starts where the eyes start.
  useLayoutEffect(() => {
    if (!open) return;
    const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>(ITEMS) ?? []);
    if (items.length === 0) return;
    setRoving(menuRef.current, items[0]!);
    items[0]!.focus();
  }, [open]);

  // Escape closes from wherever focus sits, as every implementation this replaces
  // did: a menu that only hears Escape from inside itself strands a reader whose
  // focus fell out of it. `stopPropagation` keeps the app's global shortcut layer
  // from also acting on the key.
  useLayoutEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== 'Escape') return;
      event.stopPropagation();
      setOpen(false);
      setPointer(null);
      if (triggerRef.current?.isConnected) triggerRef.current.focus();
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [open]);

  // A press outside closes. `mousedown` rather than `pointerdown`: four of the
  // five implementations this replaces used it, it is what their tests dispatch,
  // and it reaches a document listener for every pointer kind the app sees.
  // Focus is not restored here: the user's click is theirs, and the browser puts
  // focus where they pressed.
  useLayoutEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent): void => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (menuRef.current?.contains(target)) return;
      if (triggerRef.current && (triggerRef.current === target || triggerRef.current.contains(target))) return;
      setOpen(false);
      setPointer(null);
    };
    document.addEventListener('mousedown', onPointerDown);
    return () => document.removeEventListener('mousedown', onPointerDown);
  }, [open]);

  // A pointer-anchored menu's position is a snapshot; scrolling or resizing the
  // window moves the row it belongs to, so the menu goes rather than drifting
  // away from its subject.
  useLayoutEffect(() => {
    if (!open || !pointer) return;
    const dismiss = (): void => {
      setOpen(false);
      setPointer(null);
    };
    window.addEventListener('resize', dismiss);
    window.addEventListener('scroll', dismiss, true);
    return () => {
      window.removeEventListener('resize', dismiss);
      window.removeEventListener('scroll', dismiss, true);
    };
  }, [open, pointer]);

  return {
    open,
    triggerRef,
    menuRef,
    pointer,
    toggle,
    openAtPointer,
    openAt,
    close,
    invoke,
    triggerProps: {
      ref: (el: HTMLElement | null) => {
        triggerRef.current = el;
      },
      'aria-haspopup': 'menu',
      'aria-expanded': open,
      onClick: toggle,
    },
  };
}

/** The keyboard contract, shared by both placements. */
function onMenuKeyDown(menu: MenuController, event: JSX.TargetedKeyboardEvent<HTMLDivElement>): void {
  if (event.key === 'Escape') {
    event.preventDefault();
    event.stopPropagation();
    menu.close(true);
    return;
  }
  if (event.key === 'Tab') {
    // Leaving by Tab must not keep focus: restore the trigger and let the browser
    // carry on from there.
    event.preventDefault();
    menu.close(true);
    return;
  }
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
  const items = Array.from(menu.menuRef.current?.querySelectorAll<HTMLButtonElement>(ITEMS) ?? []);
  if (items.length === 0) return;
  event.preventDefault();
  const index = items.indexOf(document.activeElement as HTMLButtonElement);
  const next = event.key === 'Home'
    ? 0
    : event.key === 'End'
      ? items.length - 1
      : (index + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
  const item = items[next];
  if (!item) return;
  setRoving(menu.menuRef.current, item);
  item.focus();
}

/**
 * The menu itself. Children are `<MenuItem>`s (or any `[role="menuitem"]`
 * buttons); the wrapper supplies the role, the Escape ownership, the keyboard
 * walk and the open-focus.
 *
 * `float` renders through a portal at the stored pointer with viewport clamping;
 * without it the menu renders inline and the caller's CSS positions it, which is
 * what the trigger-anchored kebabs want and what the style contracts pin.
 */
export function Menu({
  menu,
  label,
  class: className,
  float = false,
  children,
}: {
  menu: MenuController;
  label: string;
  class?: string;
  float?: boolean;
  children: ComponentChildren;
}): JSX.Element | null {
  if (!menu.open) return null;

  const body = (
    <div
      ref={(el) => {
        menu.menuRef.current = el;
        if (el && float) clampToViewport(el, menu.pointer);
      }}
      class={float ? `menu-float${className ? ` ${className}` : ''}` : className}
      role="menu"
      aria-label={label}
      data-escape-local
      onKeyDown={(event) => onMenuKeyDown(menu, event)}
    >
      {children}
    </div>
  );

  if (!float) return body;

  return createPortal(
    <div
      class="menu-layer"
      style={menu.pointer ? { top: `${menu.pointer.y}px`, left: `${menu.pointer.x}px` } : undefined}
    >
      {body}
    </div>,
    document.body,
  );
}

/** Keep a pointer-anchored menu on screen: flip up, pull left, never off-edge. */
function clampToViewport(el: HTMLElement, pointer: Pointer | null): void {
  if (!pointer) return;
  const rect = el.getBoundingClientRect();
  const margin = 8;
  let { x, y } = pointer;
  if (y + rect.height > window.innerHeight - margin) {
    y = Math.max(margin, window.innerHeight - rect.height - margin);
  }
  if (x + rect.width > window.innerWidth - margin) {
    x = Math.max(margin, window.innerWidth - rect.width - margin);
  }
  const layer = el.parentElement;
  if (layer && layer.classList.contains('menu-layer')) {
    layer.style.top = `${y}px`;
    layer.style.left = `${x}px`;
  }
}

/**
 * A menu item: the shared `.menu-item` button. The tab index is roving and owned
 * by the controller, so exactly one item is ever in the Tab order.
 */
export function MenuItem({
  menu,
  action,
  children,
}: {
  menu: MenuController;
  action: MenuAction;
  children?: ComponentChildren;
}): JSX.Element {
  return (
    <button
      type="button"
      role="menuitem"
      tabIndex={-1}
      class={`menu-item${action.danger ? ' menu-item-danger' : ''}`}
      disabled={action.disabled}
      aria-keyshortcuts={action.keyshortcuts}
      title={action.title}
      onClick={() => menu.invoke(action.onSelect)}
    >
      {action.icon}
      {children ?? action.label}
    </button>
  );
}

/** How long a touch must rest before it means "menu" rather than "tap". */
const LONG_PRESS_MS = 450;

/** Travel that turns a resting finger into a scroll or a drag, cancelling the press.
 *  A finger is never perfectly still, so this is not zero. */
const LONG_PRESS_SLOP = 10;

interface LongPress {
  timer: number;
  x: number;
  y: number;
}

/** Press state lives on the element, not in the render closure: a row can re-render
 *  (a streaming title, an unread count) between pointerdown and pointerup, and a
 *  closure-scoped timer would be orphaned and would open a menu for a row the
 *  reader has already left. */
const presses = new WeakMap<HTMLElement, LongPress>();

function cancelLongPress(el: HTMLElement): void {
  const press = presses.get(el);
  if (!press) return;
  window.clearTimeout(press.timer);
  presses.delete(el);
}

/**
 * Drop the press events a browser synthesises after a touch ends, so a long-press
 * that opened the menu does not also run the row's own click (open the session,
 * switch the project). Scoped to events outside the menu: a reader who taps an
 * item immediately after the menu appears must still get that item.
 */
function swallowSyntheticPress(menu: MenuController, el: HTMLElement): void {
  const swallow = (event: Event): void => {
    const target = event.target;
    if (target instanceof Node && menu.menuRef.current?.contains(target)) return;
    event.stopPropagation();
    event.preventDefault();
  };
  for (const type of ['mousedown', 'mouseup', 'click'] as const) {
    el.addEventListener(type, swallow, { capture: true, once: true });
    // A browser that synthesises nothing must not leave the listener armed to eat
    // the reader's next real press.
    window.setTimeout(() => el.removeEventListener(type, swallow, true), LONG_PRESS_MS + 400);
  }
}

/**
 * Right-click wiring for a row, plus the touch equivalent.
 *
 * `contextmenu` is the desktop gesture. It never fires on iOS Safari, and only
 * fires on Android after the browser's own long-press delay, so a menu wired to
 * it alone is a desktop-only feature - which is what happened: every "copy what
 * the row shows" affordance added in `00458e5` was unreachable on a phone. The
 * primitive therefore owns the touch path too: a finger that rests for
 * `LONG_PRESS_MS` without travelling opens the same menu at the same point.
 *
 * Kept separate from the controller so a surface can add its own guard - a row
 * that is mid-rename, a selection in progress - before either gesture counts.
 */
export function contextMenuProps(
  menu: MenuController,
  allow?: () => boolean,
  /** Where focus returns on close. Defaults to the pressed element; a row
   *  usually wants its kebab, which is the trigger a keyboard user would have
   *  used and the element the row's own menu restores focus to. */
  trigger?: () => HTMLElement | null,
): {
  onContextMenu: (event: MouseEvent) => void;
  onPointerDown: (event: PointerEvent) => void;
  onPointerMove: (event: PointerEvent) => void;
  onPointerUp: (event: PointerEvent) => void;
  onPointerCancel: (event: PointerEvent) => void;
} {
  /** The press, as a point, minus anything that cannot open a menu. */
  const anchorFor = (el: HTMLElement | null): HTMLElement | null =>
    trigger?.() ?? el;

  return {
    onContextMenu: (event: MouseEvent) => {
      // Never steal right-click from a text field or an editable region: the
      // native menu's Copy and spellcheck are the affordance there.
      const target = event.target;
      if (target instanceof HTMLElement
        && target.closest('input, textarea, select, [contenteditable="true"], [role="menu"]')) return;
      if (allow && !allow()) return;
      event.preventDefault();
      const row = event.currentTarget instanceof HTMLElement ? event.currentTarget : null;
      menu.openAt(event.clientX, event.clientY, anchorFor(row));
    },

    onPointerDown: (event: PointerEvent) => {
      // A mouse keeps the native right-click: a left-press that rests is a
      // select-drag, not a menu gesture.
      if (event.pointerType === 'mouse') return;
      if (event.button !== 0) return;
      const target = event.target;
      if (target instanceof HTMLElement
        && target.closest('input, textarea, select, [contenteditable="true"], [role="menu"]')) return;
      const row = event.currentTarget;
      if (!(row instanceof HTMLElement)) return;
      cancelLongPress(row);
      const press: LongPress = { x: event.clientX, y: event.clientY, timer: 0 };
      press.timer = window.setTimeout(() => {
        presses.delete(row);
        if (allow && !allow()) return;
        swallowSyntheticPress(menu, row);
        menu.openAt(press.x, press.y, anchorFor(row));
      }, LONG_PRESS_MS);
      presses.set(row, press);
    },

    onPointerMove: (event: PointerEvent) => {
      const row = event.currentTarget;
      if (!(row instanceof HTMLElement)) return;
      const press = presses.get(row);
      if (!press) return;
      // Past the slop this is a scroll or a drag, and a menu appearing mid-scroll
      // is worse than no menu.
      if (Math.abs(event.clientX - press.x) > LONG_PRESS_SLOP
        || Math.abs(event.clientY - press.y) > LONG_PRESS_SLOP) cancelLongPress(row);
    },

    onPointerUp: (event: PointerEvent) => {
      if (event.currentTarget instanceof HTMLElement) cancelLongPress(event.currentTarget);
    },

    onPointerCancel: (event: PointerEvent) => {
      if (event.currentTarget instanceof HTMLElement) cancelLongPress(event.currentTarget);
    },
  };
}

