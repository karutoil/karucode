import type { ComponentChildren, JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { modKey } from '../lib/platform';
import * as actions from '../state/actions';
import { controlPlaneOpen } from '../state/controlPlane';
import * as store from '../state/store';
import * as workbench from '../state/workbench';
import { CopyIcon, KebabIcon, PaletteIcon, SearchIcon, SettingsIcon } from './icons';

interface WorkbenchTaskMenuProps {
  canCopy: boolean;
  onCopy: () => void;
  controlPlane?: boolean;
}

function TaskMenuItem({
  index,
  activeIndex,
  onFocus,
  onSelect,
  disabled = false,
  'aria-keyshortcuts': keyboardShortcut,
  children,
}: {
  index: number;
  activeIndex: number;
  onFocus: (index: number) => void;
  onSelect: () => void;
  disabled?: boolean;
  'aria-keyshortcuts'?: string;
  children: ComponentChildren;
}): JSX.Element {
  return (
    <button
      type="button"
      role="menuitem"
      tabIndex={index === activeIndex ? 0 : -1}
      disabled={disabled}
      aria-keyshortcuts={keyboardShortcut}
      onFocus={() => onFocus(index)}
      onClick={onSelect}
    >
      {children}
    </button>
  );
}

/** One accessible home for secondary task and workspace actions. */
export function WorkbenchTaskMenu({
  canCopy,
  onCopy,
  controlPlane = false,
}: WorkbenchTaskMenuProps): JSX.Element {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const session = store.currentSessionId.value;

  const close = (restoreFocus = false): void => {
    setOpen(false);
    if (restoreFocus) trigger.current?.focus();
  };

  useEffect(() => setOpen(false), [session]);
  useEffect(() => {
    if (!open) return;
    setActiveIndex(0);
    const menu = root.current?.querySelector<HTMLElement>('[role="menu"]');
    menu?.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus();
    const onPointerDown = (event: MouseEvent): void => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onPointerDown);
    return () => document.removeEventListener('mousedown', onPointerDown);
  }, [open]);

  const invoke = (action: () => void): void => {
    // Close before opening a dialog so the overlay's trap has a stable trigger to
    // restore to; a focused menuitem that disappears cannot receive that focus.
    trigger.current?.focus();
    close();
    action();
  };

  return (
    <div class="workbench-task-menu" ref={root}>
      <button
        ref={trigger}
        type="button"
        class="head-btn head-icon"
        aria-label="Task actions"
        title="Task actions"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        <KebabIcon size={15} />
      </button>
      {open ? (
        <div
          class="workbench-task-menu-popover"
          role="menu"
          aria-label="Task and workspace actions"
          data-escape-local
          onKeyDown={(event) => {
            const items = Array.from(root.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? []);
            if (event.key === 'Escape') {
              event.preventDefault();
              event.stopPropagation();
              close(true);
              return;
            }
            if (event.key === 'Tab') {
              event.preventDefault();
              close(true);
              return;
            }
            if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
            if (items.length === 0) return;
            event.preventDefault();
            const index = items.indexOf(document.activeElement as HTMLButtonElement);
            const next = event.key === 'Home'
              ? 0
              : event.key === 'End'
                ? items.length - 1
                : (index + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
            items[next]?.focus();
          }}
        >
          <TaskMenuItem index={0} activeIndex={activeIndex} onFocus={setActiveIndex} disabled={!canCopy} onSelect={() => invoke(onCopy)}>
            <CopyIcon size={13} /> Copy conversation
          </TaskMenuItem>
          {!controlPlane ? (
            <TaskMenuItem index={1} activeIndex={activeIndex} onFocus={setActiveIndex} onSelect={() => invoke(() => workbench.openContext('now'))}>
              <SettingsIcon size={13} /> Session context
            </TaskMenuItem>
          ) : null}
          <TaskMenuItem index={2} activeIndex={activeIndex} onFocus={setActiveIndex} aria-keyshortcuts="Control+K Meta+K" onSelect={() => invoke(actions.togglePalette)}>
            <SearchIcon size={13} /> Commands <kbd>{modKey()} K</kbd>
          </TaskMenuItem>
          <span class="workbench-task-menu-rule" role="separator" />
          <TaskMenuItem index={3} activeIndex={activeIndex} onFocus={setActiveIndex} aria-keyshortcuts="Control+Shift+T Meta+Shift+T" onSelect={() => invoke(actions.openThemeStudio)}>
            <PaletteIcon size={13} /> Theme studio
          </TaskMenuItem>
          <TaskMenuItem index={4} activeIndex={activeIndex} onFocus={setActiveIndex} onSelect={() => invoke(actions.openSettings)}>
            <SettingsIcon size={13} /> Settings
          </TaskMenuItem>
          {!controlPlane ? (
            <TaskMenuItem index={5} activeIndex={activeIndex} onFocus={setActiveIndex} onSelect={() => invoke(() => { controlPlaneOpen.value = true; })}>
              <SearchIcon size={13} /> All tasks
            </TaskMenuItem>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
