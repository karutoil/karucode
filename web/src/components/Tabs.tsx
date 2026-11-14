import { useEffect, useId, useRef } from 'preact/hooks';
import type { JSX, TargetedKeyboardEvent } from 'preact';
import { CloseIcon } from './icons';

export interface TabItem<T extends string> {
  id: T;
  label: string;
  /** Optional leading glyph (workspace tabs carry a 15px icon in the mock). */
  icon?: JSX.Element;
  /** Optional trailing count pill, e.g. the number of changed files. */
  count?: number;
  /**
   * Offer a close button beside the label. Only callers that pass `onClose` can
   * act on it; the strip is otherwise the built-in, permanently-open views.
   */
  closable?: boolean;
  /**
   * Hover text for the tab button.
   *
   * A tab label has room for one short word, so a number worth seeing without
   * opening the panel (the context fill) does not fit as a pill - a bare `32`
   * would be read as a count of something. The tooltip carries the sentence.
   */
  title?: string;
}

/**
 * `default` is the compact inspector strip; `workspace` is the header tab bar
 * (Conversation / Changes); `fileview` is the file viewer's Preview/Source
 * control. All three share the same tablist semantics and differ only in the
 * classes the stylesheets key off.
 *
 * The settings sheet used to be a fourth (`settings`, a horizontal strip). It is
 * a two-pane window now — a vertical rail, a search field and a pane header — so
 * its navigation is `SettingsNav.tsx` rather than a variant of this strip: one
 * implementation per role.
 */
export type TabVariant = 'default' | 'workspace' | 'fileview';

export interface TabsProps<T extends string> {
  items: readonly TabItem<T>[];
  active: T;
  onChange: (id: T) => void;
  /** Called by a `closable` item's close button (and by Delete/Backspace). */
  onClose?: (id: T) => void;
  label: string;
  idPrefix?: string;
  variant?: TabVariant;
}

/**
 * Keyboard-operable tablist: automatic activation, arrow keys move selection,
 * Home/End jump to the ends, and only the selected tab is in the tab order.
 *
 * The strip scrolls once plugins add more tabs than fit (see `.tabs` in
 * inspector.css), so the selected tab is also kept in view: a tab can be
 * activated without a pointer - arrow key, `Ctrl+J` restore, a plugin panel
 * arriving - and it must never be selected off-screen.
 */
export function Tabs<T extends string>({
  items,
  active,
  onChange,
  onClose,
  label,
  idPrefix,
  variant = 'default',
}: TabsProps<T>): JSX.Element {
  const generated = useId();
  const prefix = idPrefix ?? generated;
  const workspace = variant === 'workspace';
  const fileview = variant === 'fileview';
  const listClass = fileview ? 'file-view-tabs' : workspace ? 'workspace-tabs' : 'tabs';
  const tabClass = fileview ? 'file-view-tab' : workspace ? 'workspace-tab' : 'tab';
  const listRef = useRef<HTMLDivElement>(null);

  // Measured by hand rather than with `scrollIntoView`, which would also scroll
  // the page and the inspector around the strip. A tab that already fits leaves
  // `scrollLeft` untouched, so selecting a visible tab never nudges the strip;
  // jsdom reports zero rects, which makes this a no-op there.
  //
  // It also runs on resize: a strip that was wide when the tab was selected can be
  // a 390px scrollport a moment later (a phone rotation, the rail collapsing), and
  // the selected tab would then sit outside it with nothing having selected it
  // since.
  useEffect(() => {
    const reveal = (): void => {
      const list = listRef.current;
      const tab = document.getElementById(`${prefix}-tab-${active}`);
      if (!list || !tab || !list.contains(tab)) return;
      const listBox = list.getBoundingClientRect();
      const tabBox = tab.getBoundingClientRect();
      if (tabBox.left < listBox.left) list.scrollLeft -= listBox.left - tabBox.left;
      else if (tabBox.right > listBox.right) list.scrollLeft += tabBox.right - listBox.right;
    };
    reveal();
    window.addEventListener('resize', reveal);
    return () => window.removeEventListener('resize', reveal);
  }, [active, prefix, items.length]);

  const onKeyDown = (event: TargetedKeyboardEvent<HTMLDivElement>): void => {
    const index = items.findIndex((item) => item.id === active);
    if (index === -1) return;
    // Delete/Backspace closes the selected tab, the way a browser's own tab strip
    // does. Only for a tab that offers a close button, so the built-in views can
    // never be removed with a stray keystroke.
    const current = items[index];
    if ((event.key === 'Delete' || event.key === 'Backspace') && current?.closable && onClose) {
      event.preventDefault();
      onClose(current.id);
      return;
    }
    let next = index;
    if (event.key === 'ArrowRight') next = (index + 1) % items.length;
    else if (event.key === 'ArrowLeft') next = (index - 1 + items.length) % items.length;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = items.length - 1;
    else return;
    event.preventDefault();
    const target = items[next];
    if (!target) return;
    onChange(target.id);
    const tab = document.getElementById(`${prefix}-tab-${target.id}`);
    tab?.focus();
  };

  return (
    <div
      ref={listRef}
      class={listClass}
      role="tablist"
      aria-label={label}
      onKeyDown={onKeyDown}
    >
      {items.map((item) => {
        const selected = item.id === active;
        const tab = (
          <button
            key={item.id}
            id={`${prefix}-tab-${item.id}`}
            type="button"
            role="tab"
            class={`${tabClass}${selected ? ' is-active' : ''}`}
            aria-selected={selected}
            // Panels are mounted lazily, so only the selected tab can point at one.
            aria-controls={selected ? `${prefix}-panel-${item.id}` : undefined}
            tabIndex={selected ? 0 : -1}
            title={item.title}
            // The label and the count pill are separate spans with no whitespace
            // between them, so the content-derived accessible name would run the
            // two together ("Terminal1"). The label names the tab properly and
            // the count stays visible as the pill.
            aria-label={item.count !== undefined && item.count > 0 ? `${item.label} ${item.count}` : item.label}
            onClick={() => onChange(item.id)}
          >
            {item.icon ? <span class="tab-icon" aria-hidden="true">{item.icon}</span> : null}
            <span>{item.label}</span>
            {item.count !== undefined && item.count > 0 ? (
              <span class="tab-count">{item.count}</span>
            ) : null}
          </button>
        );
        if (!item.closable || !onClose) return tab;
        // A close button cannot live *inside* the tab button - a button inside a
        // button is invalid markup and the inner click would also activate the
        // tab - so a closable tab is the tab plus a sibling in a neutral wrapper.
        return (
          <div key={item.id} class="tab-slot" role="presentation">
            {tab}
            <button
              type="button"
              class="tab-close"
              aria-label={`Close ${item.label}`}
              title={`Close ${item.label}`}
              onClick={() => onClose(item.id)}
            >
              <CloseIcon size={11} />
            </button>
          </div>
        );
      })}
    </div>
  );
}

export interface TabPanelProps {
  id: string;
  idPrefix: string;
  active: boolean;
  /**
   * Keep the panel in the DOM (merely `hidden`) while another tab is selected.
   *
   * The default unmounts it, which is right for inspector panels: a plugin panel
   * costs a fetch per view. A panel that holds form state cannot be unmounted
   * without throwing that state away, so a caller that owns state inside its
   * panels turns this on.
   */
  persist?: boolean;
  children: JSX.Element | JSX.Element[] | null;
}

export function TabPanel({
  id,
  idPrefix,
  active,
  persist = false,
  children,
}: TabPanelProps): JSX.Element | null {
  if (!active && !persist) return null;
  return (
    <div
      id={`${idPrefix}-panel-${id}`}
      class="tab-panel"
      role="tabpanel"
      aria-labelledby={`${idPrefix}-tab-${id}`}
      // `hidden` takes the panel out of the tab order and the accessibility tree,
      // so a kept-alive panel is exactly as unreachable as an unmounted one.
      hidden={!active}
      tabIndex={0}
    >
      {children}
    </div>
  );
}