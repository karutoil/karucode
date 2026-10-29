import { useId } from 'preact/hooks';
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
 * (Conversation / Changes) that shares the same tablist semantics but a
 * different visual treatment.
 */
export type TabVariant = 'default' | 'workspace';

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
      class={workspace ? 'workspace-tabs' : 'tabs'}
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
            class={`${workspace ? 'workspace-tab' : 'tab'}${selected ? ' is-active' : ''}`}
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
  children: JSX.Element | JSX.Element[] | null;
}

export function TabPanel({ id, idPrefix, active, children }: TabPanelProps): JSX.Element | null {
  if (!active) return null;
  return (
    <div
      id={`${idPrefix}-panel-${id}`}
      class="tab-panel"
      role="tabpanel"
      aria-labelledby={`${idPrefix}-tab-${id}`}
      tabIndex={0}
    >
      {children}
    </div>
  );
}