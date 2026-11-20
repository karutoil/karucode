import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import type { JSX } from 'preact';
import { useState } from 'preact/hooks';
import type { WorkspaceEntry } from '../api/types';
import { copyText } from '../lib/clipboard';
import * as store from '../state/store';
import { resetAppState } from '../test/fixtures';
import { FileTree } from './FileTree';

vi.mock('../lib/clipboard', () => ({ copyText: vi.fn() }));

const copyTextMock = vi.mocked(copyText);

function entry(path: string, kind: 'file' | 'dir' = 'file'): WorkspaceEntry {
  return { name: path.split('/').pop() ?? path, path, kind, size: 1 };
}

beforeEach(() => {
  resetAppState();
  vi.clearAllMocks();
});

interface HarnessProps {
  rootEntries: WorkspaceEntry[];
  children: Record<string, WorkspaceEntry[]>;
  /** Directories open on first render; the harness owns the set after that. */
  expanded?: string[];
  loading?: string[];
  rootPath?: string;
  onToggleDir?: (entry: WorkspaceEntry) => void;
  onOpenFile?: (path: string) => void;
  onOpenFileInTab?: (path: string) => void;
}

/**
 * `FileTree` is controlled: expansion lives in the caller. This harness holds it
 * so a click really expands and really collapses, the way `WorkspaceTab` does.
 *
 * `onOpenFileInTab` has no default on purpose: `FileTree` hides that menu item
 * when the caller passes none, and a test needs to be able to pass none.
 */
function Harness({
  rootEntries,
  children,
  expanded,
  loading = [],
  rootPath = '/home/me/project',
  onToggleDir,
  onOpenFile = () => undefined,
  onOpenFileInTab,
}: HarnessProps): JSX.Element {
  const [open, setOpen] = useState<Set<string>>(() => new Set(expanded));
  const toggle = (dir: WorkspaceEntry): void => {
    onToggleDir?.(dir);
    setOpen((previous) => {
      const next = new Set(previous);
      if (next.has(dir.path)) next.delete(dir.path);
      else next.add(dir.path);
      return next;
    });
  };
  return (
    <FileTree
      rootPath={rootPath}
      rootEntries={rootEntries}
      children={children}
      expanded={open}
      loading={new Set(loading)}
      onToggleDir={toggle}
      onOpenFile={onOpenFile}
      onOpenFileInTab={onOpenFileInTab}
    />
  );
}

const BASE: HarnessProps = {
  rootEntries: [entry('README.md'), entry('src', 'dir')],
  children: { src: [entry('src/main.rs')] },
  // What `WorkspaceTab` passes, and what makes the four-item menu the default
  // these tests describe; a test that wants the three-item menu passes `undefined`.
  onOpenFileInTab: () => undefined,
};

function renderTree(overrides: Partial<HarnessProps> = {}): {
  container: HTMLElement;
  unmount: () => void;
} {
  const view = render(<Harness {...BASE} {...overrides} />);
  return { container: view.container as HTMLElement, unmount: view.unmount };
}

function row(name: string): HTMLElement {
  return screen.getByRole('treeitem', { name });
}

function rect(top: number, height = 0): DOMRect {
  return {
    top,
    bottom: top + height,
    left: 0,
    right: 0,
    width: 0,
    height,
    x: 0,
    y: top,
    toJSON: () => ({}),
  } as DOMRect;
}

/**
 * Render the tree inside a stand-in for the inspector's scroll container.
 *
 * jsdom does no layout, so nothing has a height and `getBoundingClientRect`
 * returns zeros; a windowed tree needs both. The scroller is given a
 * `clientHeight`, a real stored `scrollTop`, and a rect that follows it, and the
 * list's rect is pinned to the top of the scroll content — which is exactly the
 * relationship `FileTree` measures in a browser. One scroll event stands in for
 * the layout pass that made the scroller measurable.
 */
function renderWindowed(overrides: Partial<HarnessProps> = {}, height = 300): {
  container: HTMLElement;
  scroller: HTMLElement;
  list: HTMLElement;
} {
  const view = render(
    <div data-testid="scroller" style={{ overflowY: 'auto' }}>
      <Harness {...BASE} {...overrides} />
    </div>,
  );
  const scroller = view.container.firstElementChild as HTMLElement;
  const list = view.container.querySelector('ul.tree') as HTMLElement;
  let scrollTop = 0;
  Object.defineProperty(scroller, 'clientHeight', { configurable: true, get: () => height });
  Object.defineProperty(scroller, 'scrollTop', {
    configurable: true,
    get: () => scrollTop,
    set: (value: number) => { scrollTop = value; },
  });
  scroller.getBoundingClientRect = () => rect(0, height);
  list.getBoundingClientRect = () => rect(-scrollTop);
  fireEvent.scroll(scroller);
  return { container: view.container as HTMLElement, scroller, list };
}

/** A workspace with `count` sibling files: enough rows to cross the threshold. */
function manyFiles(count: number): WorkspaceEntry[] {
  return Array.from({ length: count }, (_, index) => entry(`file-${String(index).padStart(4, '0')}.rs`));
}

/** A chain of `levels` nested directories, every one of them expanded. */
function deepChain(levels: number): {
  rootEntries: WorkspaceEntry[];
  children: Record<string, WorkspaceEntry[]>;
  expanded: string[];
} {
  const rootEntries = [entry('d0', 'dir')];
  const children: Record<string, WorkspaceEntry[]> = {};
  const expanded: string[] = [];
  let current = 'd0';
  for (let level = 1; level <= levels; level += 1) {
    const next = `${current}/d${level}`;
    children[current] = [entry(next, 'dir'), entry(`${current}/file-${level}.md`)];
    expanded.push(current);
    current = next;
  }
  children[current] = [entry(`${current}/leaf.md`)];
  expanded.push(current);
  return { rootEntries, children, expanded };
}

describe('FileTree rows', () => {
  it('renders files and folders as treeitems with their depth and position', () => {
    renderTree();

    const file = row('README.md');
    const folder = row('src');
    expect(file.getAttribute('aria-level')).toBe('1');
    expect(folder.getAttribute('aria-level')).toBe('1');
    // A file is never expanded: the attribute is absent rather than `false`.
    expect(file.hasAttribute('aria-expanded')).toBe(false);
    expect(folder.getAttribute('aria-expanded')).toBe('false');
    // Position is explicit: a windowed tree cannot be counted by counting DOM rows.
    expect(file.getAttribute('aria-posinset')).toBe('1');
    expect(folder.getAttribute('aria-posinset')).toBe('2');
    expect(file.getAttribute('aria-setsize')).toBe('2');
    expect(folder.getAttribute('aria-setsize')).toBe('2');
    expect(folder.getAttribute('data-tree-path')).toBe('src');
  });

  it('expands and collapses a folder from its own row and reports the toggle', async () => {
    const onToggleDir = vi.fn();
    const { container } = renderTree({ onToggleDir });
    expect(screen.queryByRole('treeitem', { name: 'main.rs' })).toBeNull();

    fireEvent.click(row('src'));
    const nested = await screen.findByRole('treeitem', { name: 'main.rs' });
    expect(onToggleDir).toHaveBeenCalledWith(expect.objectContaining({ path: 'src', kind: 'dir' }));
    expect(nested.getAttribute('aria-level')).toBe('2');
    expect(row('src').getAttribute('aria-expanded')).toBe('true');
    expect(container.querySelector('[data-tree-path="src/main.rs"]')).toBe(nested);

    fireEvent.click(row('src'));
    await waitFor(() => expect(screen.queryByRole('treeitem', { name: 'main.rs' })).toBeNull());
    expect(row('src').getAttribute('aria-expanded')).toBe('false');
  });

  it('opens a file on click and from Enter and Space', () => {
    const onOpenFile = vi.fn();
    const { container } = renderTree({ onOpenFile });
    const tree = screen.getByRole('tree', { name: /Workspace files in/ });

    fireEvent.click(row('README.md'));
    expect(onOpenFile).toHaveBeenCalledWith('README.md');

    // `preact/compat` turns the row's `onFocus` into a `focusin` listener, so
    // that is the event a test has to send to move the roving tabindex.
    fireEvent.focusIn(row('README.md'));
    fireEvent.keyDown(tree, { key: 'Enter' });
    expect(onOpenFile).toHaveBeenLastCalledWith('README.md');
    fireEvent.keyDown(tree, { key: ' ' });
    expect(onOpenFile).toHaveBeenCalledTimes(3);
    // A folder's Enter expands rather than opening a file.
    fireEvent.focusIn(row('src'));
    fireEvent.keyDown(tree, { key: 'Enter' });
    expect(container.querySelector('[data-tree-path="src/main.rs"]')).not.toBeNull();
    expect(onOpenFile).toHaveBeenCalledTimes(3);
  });

  it('caps the indent so a deeply nested row keeps its name on screen', () => {
    const chain = deepChain(12);
    const { container } = renderTree(chain);

    const deepest = container.querySelector<HTMLElement>('[data-tree-path$="/leaf.md"]');
    expect(deepest?.getAttribute('aria-level')).toBe('14');
    // `Math.min(depth, 8) * 12 + 8`: 12px per level, never past eight levels.
    expect(container.querySelector<HTMLElement>('[data-tree-path="d0"]')?.style.paddingLeft)
      .toBe('8px');
    expect(container.querySelector<HTMLElement>('[data-tree-path="d0/d1"]')?.style.paddingLeft)
      .toBe('20px');
    expect(deepest?.style.paddingLeft).toBe('104px');
  });
});

describe('FileTree context menu', () => {
  it('opens one shared menu on right-click and returns focus to the pressed row', () => {
    renderTree();
    const file = row('README.md');

    // `fireEvent` returns false when the event was cancelled: the row's menu
    // replaces the browser's, so the browser's must not also appear.
    expect(fireEvent.contextMenu(file, { clientX: 40, clientY: 60 })).toBe(false);

    const menu = screen.getByRole('menu', { name: 'File actions for README.md' });
    expect(within(menu).getAllByRole('menuitem').map((item) => item.textContent))
      .toEqual(['Open', 'Open in a tab', 'Copy path', 'Copy relative path']);
    // Opening focuses the first item, and the menu is portalled onto the menu
    // layer, where no scroller in the inspector can clip it.
    expect(document.activeElement).toBe(within(menu).getByRole('menuitem', { name: 'Open' }));
    expect(menu.closest('.menu-layer')?.parentElement).toBe(document.body);

    fireEvent.keyDown(menu, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    expect(document.activeElement).toBe(file);
  });

  it('is a single instance: a second right-click reuses it instead of stacking one per row', () => {
    renderTree();
    fireEvent.contextMenu(row('README.md'), { clientX: 40, clientY: 60 });
    expect(document.querySelectorAll('[role="menu"]')).toHaveLength(1);

    // Without closing the first, the folder's menu replaces it in place.
    fireEvent.contextMenu(row('src'), { clientX: 10, clientY: 10 });
    expect(document.querySelectorAll('[role="menu"]')).toHaveLength(1);
    const menu = screen.getByRole('menu', { name: 'Folder actions for src' });
    expect(within(menu).queryByRole('menuitem', { name: 'Open' })).toBeNull();
    expect(within(menu).getAllByRole('menuitem').map((item) => item.textContent))
      .toEqual(['Copy path', 'Copy relative path']);

    // Escape restores focus to the row the menu now describes, not the first one.
    fireEvent.keyDown(menu, { key: 'Escape' });
    expect(document.activeElement).toBe(row('src'));
  });

  it('copies the path of the row the shared menu describes', async () => {
    copyTextMock.mockResolvedValue(true);
    renderTree();

    fireEvent.contextMenu(row('README.md'), { clientX: 40, clientY: 60 });
    // The menu now points at `src`; "Copy path" must not still mean README.md.
    fireEvent.contextMenu(row('src'), { clientX: 10, clientY: 10 });
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy path' }));

    await waitFor(() => expect(copyTextMock).toHaveBeenCalledWith('/home/me/project/src'));
    expect(screen.queryByRole('menu')).toBeNull();
    expect(store.toasts.value.at(-1)).toMatchObject({ message: 'Path copied.', tone: 'success' });
  });

  it('copies the joined absolute path and the workspace-relative one from a nested row', async () => {
    copyTextMock.mockResolvedValue(true);
    renderTree();

    fireEvent.click(row('src'));
    const nested = await screen.findByRole('treeitem', { name: 'main.rs' });

    fireEvent.contextMenu(nested, { clientX: 40, clientY: 60 });
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy path' }));
    await waitFor(() => expect(copyTextMock).toHaveBeenCalledWith('/home/me/project/src/main.rs'));
    expect(store.toasts.value.at(-1)).toMatchObject({ message: 'Path copied.', tone: 'success' });

    fireEvent.contextMenu(nested, { clientX: 40, clientY: 60 });
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy relative path' }));
    await waitFor(() => expect(copyTextMock).toHaveBeenLastCalledWith('src/main.rs'));
    expect(store.toasts.value.at(-1)).toMatchObject({
      message: 'Relative path copied.',
      tone: 'success',
    });
  });

  it('says so when the clipboard refuses the copy', async () => {
    copyTextMock.mockResolvedValue(false);
    renderTree();

    fireEvent.contextMenu(row('README.md'), { clientX: 40, clientY: 60 });
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy path' }));

    await waitFor(() =>
      expect(store.toasts.value.at(-1)).toMatchObject({
        message: 'Could not copy the path.',
        tone: 'error',
      }),
    );
  });

  it('opens the row in a tab', async () => {
    const onOpenFileInTab = vi.fn();
    renderTree({ onOpenFileInTab });

    fireEvent.click(row('src'));
    const nested = await screen.findByRole('treeitem', { name: 'main.rs' });
    fireEvent.contextMenu(nested, { clientX: 40, clientY: 60 });
    fireEvent.click(screen.getByRole('menuitem', { name: 'Open in a tab' }));

    expect(onOpenFileInTab).toHaveBeenCalledWith('src/main.rs');
    expect(screen.queryByRole('menu')).toBeNull();
  });

  it('hides the tab item when the caller has no tab to open into', () => {
    renderTree({ onOpenFileInTab: undefined });
    fireEvent.contextMenu(row('README.md'), { clientX: 40, clientY: 60 });

    const menu = screen.getByRole('menu', { name: 'File actions for README.md' });
    expect(within(menu).getAllByRole('menuitem').map((item) => item.textContent))
      .toEqual(['Open', 'Copy path', 'Copy relative path']);
  });

  it('drops the second copy item instead of inventing a path when no workspace is known', () => {
    // `WorkspaceTab` hands the tree `'.'` when nothing registered itself; joining
    // a relative path onto that is not an absolute path, so the menu says less
    // rather than something false.
    renderTree({ rootPath: '.' });
    fireEvent.contextMenu(row('README.md'), { clientX: 40, clientY: 60 });

    const menu = screen.getByRole('menu', { name: 'File actions for README.md' });
    expect(within(menu).getAllByRole('menuitem').map((item) => item.textContent))
      .toEqual(['Open', 'Open in a tab', 'Copy path']);
    expect(within(menu).getByRole('menuitem', { name: 'Copy path' }).getAttribute('title'))
      .toBe('README.md');
  });
});

describe('FileTree windowing', () => {
  it('renders every row when the scroll container has no measured height', () => {
    // jsdom gives nothing a height: the tree must fall back to mounting the whole
    // listing rather than guess a viewport and hide rows it cannot measure.
    const { container } = renderTree({ rootEntries: manyFiles(1500) });

    expect(screen.getAllByRole('treeitem')).toHaveLength(1500);
    expect(container.querySelectorAll('.tree-spacer')).toHaveLength(0);
  });

  it('renders every row at exactly the threshold, and windows one row past it', () => {
    const atThreshold = renderTree({ rootEntries: manyFiles(150) });
    expect(screen.getAllByRole('treeitem')).toHaveLength(150);
    expect(atThreshold.container.querySelectorAll('.tree-spacer')).toHaveLength(0);
    atThreshold.unmount();

    const past = renderWindowed({ rootEntries: manyFiles(151) });
    expect(screen.getAllByRole('treeitem').length).toBeLessThan(151);
    expect(past.container.querySelectorAll('.tree-spacer').length).toBeGreaterThan(0);
  });

  it('mounts only the visible rows plus overscan, with spacers for the rest', () => {
    const { container } = renderWindowed({ rootEntries: manyFiles(1500) });

    // 300px of scroller at a 29px row height is 12 rows on screen, plus the
    // 8-row overscan below; nothing above, because the list starts at the top.
    const mounted = screen.getAllByRole('treeitem');
    expect(mounted).toHaveLength(20);
    expect(mounted[0]?.getAttribute('data-tree-path')).toBe('file-0000.rs');
    expect(mounted[0]?.getAttribute('aria-posinset')).toBe('1');
    expect(mounted[0]?.getAttribute('aria-setsize')).toBe('1500');

    const spacers = Array.from(container.querySelectorAll<HTMLElement>('.tree-spacer'));
    expect(spacers).toHaveLength(1);
    expect(spacers[0]?.getAttribute('role')).toBe('presentation');
    expect(spacers[0]?.getAttribute('aria-hidden')).toBe('true');
    // The whole listing is still accounted for by the spacers.
    expect(spacers[0]?.style.height).toBe(`${(1500 - 20) * 29}px`);
  });

  it('follows the row height the stylesheet publishes, so a thumb-sized row moves the window with it', () => {
    // `--tree-row-h` is 29px and 44px under `pointer: coarse` (inspector.css), and
    // `FileTree` reads it instead of hardcoding 29: 300px of scroller at 44px is
    // ceil(300 / 44) + 1 = 8 rows on screen, plus the 8-row overscan below, and the
    // spacer has to be 44px per remaining row or the scrollbar lies.
    const { container, scroller } = renderWindowed({ rootEntries: manyFiles(1500) });
    const list = container.querySelector('ul.tree') as HTMLElement;
    const real = window.getComputedStyle.bind(window);
    // Only the tree's own computed style is patched: the scroller lookup itself
    // reads `overflow-y` off the same elements, so faking the whole function would
    // stop the tree from finding a scroll parent at all.
    const spy = vi.spyOn(window, 'getComputedStyle').mockImplementation(((element: Element, pseudo?: string | null) => {
      const style = real(element, pseudo);
      if (element === list) {
        const read = style.getPropertyValue.bind(style);
        style.getPropertyValue = (name: string) => (name === '--tree-row-h' ? '44px' : read(name));
      }
      return style;
    }) as typeof window.getComputedStyle);

    fireEvent.scroll(scroller);

    const mounted = screen.getAllByRole('treeitem');
    expect(mounted).toHaveLength(16);
    expect(mounted[0]?.getAttribute('data-tree-path')).toBe('file-0000.rs');
    const spacers = Array.from(container.querySelectorAll<HTMLElement>('.tree-spacer'));
    expect(spacers[0]?.style.height).toBe(`${(1500 - 16) * 44}px`);
    spy.mockRestore();
  });

  it('moves the window with the scroller', () => {
    const { container, scroller } = renderWindowed({ rootEntries: manyFiles(1500) });

    scroller.scrollTop = 500 * 29;
    fireEvent.scroll(scroller);

    const mounted = screen.getAllByRole('treeitem');
    // First row on screen is 500, minus the 8-row overscan above.
    expect(mounted[0]?.getAttribute('data-tree-path')).toBe('file-0492.rs');
    expect(mounted).toHaveLength(28);
    const spacers = Array.from(container.querySelectorAll<HTMLElement>('.tree-spacer'));
    expect(spacers).toHaveLength(2);
    expect(spacers[0]?.style.height).toBe(`${492 * 29}px`);
    expect(spacers[1]?.style.height).toBe(`${(1500 - 520) * 29}px`);
  });

  it('keeps ArrowDown, End and Home working across the edge of the window', () => {
    const { list, scroller } = renderWindowed({ rootEntries: manyFiles(1500) });
    expect(screen.getAllByRole('treeitem')).toHaveLength(20);

    // Twenty presses walk off the mounted edge: row 20 is not in the DOM, so the
    // tree has to scroll it into the window before it can take focus.
    for (let press = 0; press < 20; press += 1) fireEvent.keyDown(list, { key: 'ArrowDown' });
    expect(screen.getByRole('treeitem', { name: 'file-0020.rs' })).toBe(document.activeElement);
    expect(scroller.scrollTop).toBe(20 * 29 + 29 - 300 + 29);

    fireEvent.keyDown(list, { key: 'End' });
    expect(screen.getByRole('treeitem', { name: 'file-1499.rs' })).toBe(document.activeElement);

    fireEvent.keyDown(list, { key: 'Home' });
    expect(screen.getByRole('treeitem', { name: 'file-0000.rs' })).toBe(document.activeElement);
    expect(scroller.scrollTop).toBe(0);
  });

  it('keeps ArrowUp working when the target row is above the window', () => {
    const { list, scroller } = renderWindowed({ rootEntries: manyFiles(1500) });
    fireEvent.keyDown(list, { key: 'End' });
    expect(screen.getByRole('treeitem', { name: 'file-1499.rs' })).toBe(document.activeElement);

    fireEvent.keyDown(list, { key: 'ArrowUp' });
    expect(screen.getByRole('treeitem', { name: 'file-1498.rs' })).toBe(document.activeElement);
    expect(scroller.scrollTop).toBeGreaterThan(0);
  });
});
