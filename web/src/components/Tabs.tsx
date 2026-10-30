import { useEffect, useId, useRef } from 'preact/hooks';
import type { JSX } from 'preact';

export interface TabItem<T extends string> {
  id: T;
  label: string;
  /** Optional leading glyph (workspace tabs carry a 15px icon in the mock). */
  icon?: JSX.Element;
  /** Optional trailing count pill, e.g. the number of changed files. */
  count?: number;
}

/**
 * `default` is the compact inspector strip; `workspace` is the header tab bar
 * (Conversation / Changes); `settings` is the settings sheet's section strip.
 * All three share the same tablist semantics and differ only in the classes the
 * stylesheets key off.
 */
export type TabVariant = 'default' | 'workspace' | 'settings';

export interface TabsProps<T extends string> {
  items: readonly TabItem<T>[];
  active: T;
  onChange: (id: T) => void;
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
  label,
  idPrefix,
  variant = 'default',
}: TabsProps<T>): JSX.Element {
  const generated = useId();
  const prefix = idPrefix ?? generated;
  const workspace = variant === 'workspace';
  const settings = variant === 'settings';
  const listClass = settings ? 'settings-tabs' : workspace ? 'workspace-tabs' : 'tabs';
  const tabClass = settings ? 'settings-tab' : workspace ? 'workspace-tab' : 'tab';
  const listRef = useRef<HTMLDivElement>(null);

  // Measured by hand rather than with `scrollIntoView`, which would also scroll
  // the page and the inspector around the strip. A tab that already fits leaves
  // `scrollLeft` untouched, so selecting a visible tab never nudges the strip;
  // jsdom reports zero rects, which makes this a no-op there.
  useEffect(() => {
    const list = listRef.current;
    const tab = document.getElementById(`${prefix}-tab-${active}`);
    if (!list || !tab || !list.contains(tab)) return;
    const listBox = list.getBoundingClientRect();
    const tabBox = tab.getBoundingClientRect();
    if (tabBox.left < listBox.left) list.scrollLeft -= listBox.left - tabBox.left;
    else if (tabBox.right > listBox.right) list.scrollLeft += tabBox.right - listBox.right;
  }, [active, prefix, items.length]);

  const onKeyDown = (event: JSX.TargetedKeyboardEvent<HTMLDivElement>): void => {
    const index = items.findIndex((item) => item.id === active);
    if (index === -1) return;
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
        return (
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
            onClick={() => onChange(item.id)}
          >
            {item.icon ? <span class="tab-icon" aria-hidden="true">{item.icon}</span> : null}
            <span>{item.label}</span>
            {item.count !== undefined && item.count > 0 ? (
              <span class="tab-count">{item.count}</span>
            ) : null}
          </button>
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