import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import type { JSX } from 'preact';
import { contextMenuProps, Menu, MenuItem, useMenu } from './menu';
import type { MenuAction } from './menu';

/**
 * The menu primitive's contract, pinned where the five refactored menus and the
 * nine right-click surfaces all inherit it from. Anything asserted here is a
 * behaviour every surface gets for free, so a regression in one place fails here
 * instead of in a surface's own suite.
 */

const ACTIONS: MenuAction[] = [
  { label: 'First', onSelect: vi.fn() },
  { label: 'Second', onSelect: vi.fn() },
  { label: 'Broken', onSelect: vi.fn(), disabled: true },
  { label: 'Last', onSelect: vi.fn(), danger: true },
];

function Harness({ float = false }: { float?: boolean }): JSX.Element {
  const menu = useMenu();
  return (
    <div>
      <button type="button" aria-label="Row actions" {...menu.triggerProps}>
        kebab
      </button>
      <div {...contextMenuProps(menu)}>
        <span>row body</span>
        <input aria-label="Row title" />
      </div>
      <Menu menu={menu} label="Test actions" float={float} class={float ? undefined : 'session-menu'}>
        {ACTIONS.map((action) => (
          <MenuItem key={action.label} menu={menu} action={action} />
        ))}
      </Menu>
    </div>
  );
}

const open = (): HTMLElement => {
  fireEvent.click(screen.getByRole('button', { name: 'Row actions' }));
  return screen.getByRole('menu');
};

describe('the menu primitive', () => {
  it('opens onto the first enabled item and owns its Escape', () => {
    render(<Harness />);
    const menu = open();
    expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'First' }));
    // The dialog layer defers to a control that owns Escape.
    expect(menu.getAttribute('data-escape-local')).toBe('true');
    expect(menu.getAttribute('aria-label')).toBe('Test actions');

    fireEvent.keyDown(menu, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Row actions' }));
  });

  it('walks enabled items with the arrows, Home and End, wrapping', () => {
    render(<Harness />);
    const menu = open();
    const names = (): (string | null)[] =>
      screen.getAllByRole('menuitem').map((item) => item.getAttribute('disabled') === null ? item.textContent : null);
    expect(names()).toEqual(['First', 'Second', null, 'Last']);

    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'Second' }));
    // The disabled item is skipped, not focused-and-stuck.
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'Last' }));
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'First' }));
    fireEvent.keyDown(menu, { key: 'ArrowUp' });
    expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'Last' }));
    fireEvent.keyDown(menu, { key: 'Home' });
    expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'First' }));
    fireEvent.keyDown(menu, { key: 'End' });
    expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'Last' }));
  });

  it('keeps exactly one item in the tab order', () => {
    render(<Harness />);
    open();
    const items = screen.getAllByRole('menuitem');
    expect(items.filter((item) => item.tabIndex === 0)).toHaveLength(1);
    fireEvent.keyDown(screen.getByRole('menu'), { key: 'End' });
    expect(items.filter((item) => item.tabIndex === 0)).toHaveLength(1);
    expect(screen.getByRole('menuitem', { name: 'Last' }).tabIndex).toBe(0);
  });

  it('gives focus back to the trigger on Tab instead of leaking it', () => {
    render(<Harness />);
    const menu = open();
    fireEvent.keyDown(menu, { key: 'Tab' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Row actions' }));
  });

  it('closes on an outside press without stealing focus', () => {
    render(
      <div>
        <Harness />
        <button type="button" aria-label="Elsewhere">elsewhere</button>
      </div>,
    );
    open();
    const elsewhere = screen.getByRole('button', { name: 'Elsewhere' });
    fireEvent.mouseDown(elsewhere);
    expect(screen.queryByRole('menu')).toBeNull();
    // The click was the user's, not the menu's: focus stays where they put it.
    expect(document.activeElement).not.toBe(screen.getByRole('button', { name: 'Row actions' }));
  });

  it('runs an item with the trigger focused first, so dialogs get a live return target', () => {
    let activeDuringAction: Element | null = null;
    const action = vi.fn(() => {
      activeDuringAction = document.activeElement;
    });
    function One(): JSX.Element {
      const menu = useMenu();
      return (
        <div>
          <button type="button" aria-label="Row actions" {...menu.triggerProps}>kebab</button>
          <Menu menu={menu} label="One action">
            <MenuItem menu={menu} action={{ label: 'Do it', onSelect: action }} />
          </Menu>
        </div>
      );
    }
    render(<One />);
    open();
    fireEvent.click(screen.getByRole('menuitem', { name: 'Do it' }));
    expect(action).toHaveBeenCalledTimes(1);
    expect(activeDuringAction).toBe(screen.getByRole('button', { name: 'Row actions' }));
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('anchors a right-click at the pointer, in a portal, and suppresses the native menu', () => {
    render(<Harness float />);
    const row = screen.getByText('row body').parentElement as HTMLElement;
    // fireEvent wraps the dispatch in act(), so the portal is on screen by the
    // time the assertion runs; a raw dispatchEvent leaves the update pending.
    const prevented = !fireEvent.contextMenu(row, { clientX: 120, clientY: 340 });
    expect(prevented).toBe(true);

    const layer = document.body.querySelector('.menu-layer') as HTMLElement | null;
    expect(layer).not.toBeNull();
    expect(layer?.style.top).toBe('340px');
    expect(layer?.style.left).toBe('120px');
    // Portalled out of the row, so no rail's overflow can clip it.
    expect(screen.getByRole('menu').closest('.menu-layer')?.parentElement).toBe(document.body);
    expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'First' }));
  });

  it('never steals right-click from a field inside the row', () => {
    render(<Harness float />);
    const field = screen.getByRole('textbox', { name: 'Row title' });
    const event = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });
    const prevented = !field.dispatchEvent(event);
    expect(prevented).toBe(false);
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('lets a surface decline a right-click before the native menu is suppressed', () => {
    function Guarded(): JSX.Element {
      const menu = useMenu();
      return (
        <div>
          <Menu menu={menu} label="Guarded">
            <MenuItem menu={menu} action={{ label: 'Never', onSelect: vi.fn() }} />
          </Menu>
          <div {...contextMenuProps(menu, () => false)}>
            <span>declined row</span>
          </div>
        </div>
      );
    }
    render(<Guarded />);
    const row = screen.getByText('declined row').parentElement as HTMLElement;
    const event = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });
    const prevented = !row.dispatchEvent(event);
    expect(prevented).toBe(false);
    expect(screen.queryByRole('menu')).toBeNull();
  });
});
