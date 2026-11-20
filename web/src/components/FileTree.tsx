import type { JSX } from 'preact';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import type { WorkspaceEntry } from '../api/types';
import { contextMenuProps, Menu, MenuItem, useMenu } from '../lib/menu';
import { copyText } from '../lib/clipboard';
import * as store from '../state/store';
import { ArrowRightIcon, ChevronRightIcon, FileCodeIcon, FileIcon, FolderIcon, FolderOpenIcon } from './icons';

export interface FileTreeProps {
  rootPath: string;
  rootEntries: WorkspaceEntry[];
  children: Record<string, WorkspaceEntry[]>;
  expanded: Set<string>;
  loading: Set<string>;
  onToggleDir: (entry: WorkspaceEntry) => void;
  /** Open a file in the viewer dialog (a click, or Enter on the row). */
  onOpenFile: (path: string) => void;
  /** Open a file as a tab in the centre pane; omitted hides that menu item. */
  onOpenFileInTab?: (path: string) => void;
}

interface FlatNode {
  entry: WorkspaceEntry;
  depth: number;
}

type FileKind = 'folder' | 'rust' | 'ts' | 'md' | 'file';

/** Guards against a server listing that claims a directory is its own child. */
const MAX_TREE_DEPTH = 32;

/**
 * One row's outer height, in CSS pixels, when the stylesheet cannot be read.
 *
 * Windowed rendering turns a row index into a scroll offset, so the number has to
 * agree with `.tree-row` in `styles/inspector.css`. It is *published* there as
 * `--tree-row-h` on `.tree` (`29px`, `44px` under `pointer: coarse`), and
 * `rowHeight()` reads it: a constant here would be a second copy that drifts the
 * moment the coarse tier moves the rows (WB-04), and every scroll step would
 * mount the wrong slice.
 *
 * The fallback is the pointer-fine number because the suite runs with CSS
 * disabled (`css: false`) and jsdom computes no custom property.
 */
const ROW_HEIGHT_FALLBACK = 29;

/** The row height the tree is actually laid out with, read from the stylesheet. */
function rowHeight(list: HTMLElement | null): number {
  if (list === null || typeof window === 'undefined' || typeof window.getComputedStyle !== 'function') {
    return ROW_HEIGHT_FALLBACK;
  }
  const raw = window.getComputedStyle(list).getPropertyValue('--tree-row-h').trim();
  const parsed = Number.parseFloat(raw);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : ROW_HEIGHT_FALLBACK;
}

/**
 * Above this many visible rows the tree mounts a window instead of everything.
 *
 * A workspace with a few hundred expanded files used to mount a `treeitem` per
 * file — each one carrying its own menu controller, what used to be a hook and
 * two pieces of state per row. Below the threshold the whole list is cheaper
 * than the bookkeeping, and mounting everything also means a tree rendered with
 * no layout yet (`clientHeight === 0`, which is every jsdom test) still shows
 * every row.
 */
const WINDOW_THRESHOLD = 150;

/** Rows kept mounted on each side of the viewport, so fast scrolling does not
 *  paint a blank band before the next scroll event lands. */
const OVERSCAN = 8;

/** The handlers that wire a row to the tree's one menu. */
type RowMenuProps = ReturnType<typeof contextMenuProps>;

interface RowWindow {
  /** First flat-list index mounted. */
  start: number;
  /** One past the last flat-list index mounted. */
  end: number;
}

function fileKind(name: string): FileKind {
  const dot = name.lastIndexOf('.');
  const ext = dot <= 0 ? '' : name.slice(dot + 1).toLowerCase();
  if (ext === 'rs') return 'rust';
  if (ext === 'ts' || ext === 'tsx' || ext === 'js' || ext === 'jsx' || ext === 'mjs') return 'ts';
  if (ext === 'md' || ext === 'mdx') return 'md';
  return 'file';
}

function iconFor(kind: FileKind, open = false): JSX.Element {
  if (kind === 'folder') return open ? <FolderOpenIcon size={14} /> : <FolderIcon size={14} />;
  if (kind === 'rust' || kind === 'ts') return <FileCodeIcon size={14} />;
  return <FileIcon size={14} />;
}

function flatten(
  entries: WorkspaceEntry[],
  depth: number,
  children: Record<string, WorkspaceEntry[]>,
  expanded: Set<string>,
  seen: Set<string> = new Set(),
): FlatNode[] {
  if (depth > MAX_TREE_DEPTH) return [];
  const result: FlatNode[] = [];
  for (const entry of entries) {
    const key = `${entry.kind}:${entry.path}`;
    // A listing that repeats a path (or points a directory at itself) would
    // otherwise recurse forever.
    if (seen.has(key)) continue;
    seen.add(key);
    result.push({ entry, depth });
    if (entry.kind === 'dir' && expanded.has(entry.path)) {
      const childEntries = children[entry.path] ?? [];
      result.push(...flatten(childEntries, depth + 1, children, expanded, seen));
    }
  }
  return result;
}

/** Is this a path the operating system resolves from the root rather than from a cwd? */
function isAbsoluteRoot(path: string): boolean {
  return path.startsWith('/') || /^[A-Za-z]:[\\/]/.test(path);
}

/**
 * The absolute form of a tree entry's path, or `null` when it cannot be derived
 * honestly from the props this tree was given.
 *
 * The server's listing is workspace-relative (`GET /api/workspace/tree` reports
 * `path` through `Workspace::relative`), and `rootPath` is the project's own path,
 * so the join is real — but only while the root is absolute. `WorkspaceTab` falls
 * back to `'.'` when no project, health or config workspace is known, and joining
 * onto that would hand the reader a path that resolves against nothing. Rather
 * than invent one, the caller omits the item that would have shown it.
 */
function absolutePath(rootPath: string, path: string): string | null {
  if (path === '') return null;
  // Already absolute: the row's own spelling is the full path.
  if (isAbsoluteRoot(path)) return path;
  if (!isAbsoluteRoot(rootPath)) return null;
  const root = rootPath.replace(/\/+$/, '');
  const rest = path.replace(/^\.\/+/, '');
  if (rest === '') return root === '' ? '/' : root;
  return `${root}/${rest}`;
}

/**
 * The nearest ancestor that scrolls vertically, or `null`.
 *
 * The tree does not own its scroller: `WorkspaceTab` renders it inside the
 * inspector's `.tab-panel`, which is the element that actually scrolls, and any
 * wrapper in between could be the one. Windowed rendering needs that element's
 * height and scroll offset, so it is found rather than assumed — and "not found"
 * (jsdom, where no stylesheet is applied and nothing has a height) simply means
 * the tree mounts every row, which is what the tests and a layout-less render
 * want.
 */
function findScrollParent(el: HTMLElement | null): HTMLElement | null {
  for (let node = el?.parentElement ?? null; node; node = node.parentElement) {
    const overflowY = window.getComputedStyle(node).overflowY;
    if (overflowY === 'auto' || overflowY === 'scroll' || overflowY === 'overlay') return node;
  }
  return null;
}

interface TreeRowProps {
  node: FlatNode;
  /** Position in the flat list, for `aria-posinset` and the window's bookkeeping. */
  index: number;
  /** Size of the whole flat list, for `aria-setsize`. */
  total: number;
  expanded: boolean;
  loading: boolean;
  tabbable: boolean;
  selected: boolean;
  /** Right-click and touch long-press, wired to the tree's single menu. */
  menuProps: RowMenuProps;
  onActivate: (entry: WorkspaceEntry) => void;
  onFocusRow: (path: string) => void;
}

/**
 * One tree row: the `treeitem`, and nothing else.
 *
 * Rows are presentational on purpose. The context menu they open belongs to
 * `FileTree`, which owns exactly one `useMenu()` for the whole tree — a menu per
 * row was a controller, a piece of open state and a pointer snapshot per
 * `treeitem`, so a thousand expanded files meant a thousand of each before a
 * single one was opened. A row now only reports *which* node was pressed; the
 * menu opens at the pointer, and focus returns to the pressed row (there is no
 * kebab to return to, and a tree of hundreds of rows would be hundreds of
 * buttons), because `contextMenuProps` falls back to the event's `currentTarget`.
 *
 * Nothing here is the only path to an action. A file still opens on click and on
 * Enter/Space, a directory still expands the same way, and the path is in the
 * row's `title`. The menu adds the two things the row never had room for: opening
 * without moving the pointer to it, and copying a path into a terminal command.
 */
function TreeRow({
  node,
  index,
  total,
  expanded,
  loading,
  tabbable,
  selected,
  menuProps,
  onActivate,
  onFocusRow,
}: TreeRowProps): JSX.Element {
  const { entry, depth } = node;
  const isDir = entry.kind === 'dir';
  const kind: FileKind = isDir ? 'folder' : fileKind(entry.name);

  return (
    <li
      role="treeitem"
      aria-level={depth + 1}
      aria-expanded={isDir ? expanded : undefined}
      aria-selected={selected}
      /* A tree that mounts only the rows in view still has to tell assistive
       * technology where each row sits: the browser can only count the rows that
       * are in the DOM, which after windowing is not the tree. */
      aria-posinset={index + 1}
      aria-setsize={total}
      data-tree-path={entry.path}
      tabIndex={tabbable ? 0 : -1}
      class={`tree-row${isDir ? ' is-dir' : ' is-file'}${expanded ? ' is-open' : ''}${
        selected ? ' is-selected' : ''
      }`}
      /* Indent per level, but capped.
       *
       * `depth * 18 + 8` is unbounded, and `MAX_TREE_DEPTH` is 32: a deeply
       * nested file reached 584px of left padding inside a ~293px inspector,
       * so its name was pushed off the row entirely and the row could not be
       * hit. Past a few levels the cue stops being useful anyway — the eye
       * reads depth from the disclosure twigs, not from a wider gutter. */
      style={{ paddingLeft: `${Math.min(depth, 8) * 12 + 8}px` }}
      title={entry.path}
      onClick={() => {
        onFocusRow(entry.path);
        onActivate(entry);
      }}
      onFocus={() => onFocusRow(entry.path)}
      {...menuProps}
    >
      <span class={`tree-chevron${expanded ? ' is-open' : ''}`} aria-hidden="true">
        {isDir ? <ChevronRightIcon size={12} /> : null}
      </span>
      <span class={`tree-icon kind-${kind}`} aria-hidden="true">
        {iconFor(kind, expanded)}
      </span>
      <span class="tree-name">{entry.name}</span>
      {loading ? <span class="tree-loading">loading…</span> : null}
      {selected ? (
        <span class="tree-open-hint" aria-hidden="true">
          <ArrowRightIcon size={12} />
        </span>
      ) : null}
    </li>
  );
}

/** Keyboard-operable tree (roving tabindex; Arrow keys, Home/End, Enter/Space). */
export function FileTree({
  rootPath,
  rootEntries,
  children,
  expanded,
  loading,
  onToggleDir,
  onOpenFile,
  onOpenFileInTab,
}: FileTreeProps): JSX.Element {
  const [focusedPath, setFocusedPath] = useState<string | null>(null);
  const container = useRef<HTMLUListElement>(null);

  // One menu for the whole tree. `menuNode` is the row it describes: the row that
  // was right-clicked (or long-pressed) most recently.
  const menu = useMenu();
  const [menuNode, setMenuNode] = useState<FlatNode | null>(null);
  /** The path `menuNode` holds, so a second press on the same row is not a
   *  state write — a touch press arms the menu on `pointerdown`, before the
   *  long-press timer even fires. */
  const armedPath = useRef<string | null>(null);

  // Rebuilt only when the listing, a loaded directory or the expansion set
  // changes: `flatten` walks every expanded directory, so running it on each
  // render made every keystroke in the tree cost a walk of the whole listing.
  const visible = useMemo(
    () => flatten(rootEntries, 0, children, expanded),
    [rootEntries, children, expanded],
  );

  /** The window, or `null` while the list is not windowed. */
  const [rowWindow, setRowWindow] = useState<RowWindow | null>(null);
  /** The laid-out row height, re-read on every measure so a pointer change (or a
   *  density change) moves the spacers with the rows. */
  const [rowPx, setRowPx] = useState(ROW_HEIGHT_FALLBACK);
  const scroller = useRef<HTMLElement | null>(null);
  /** Read by `measure`, which must not be re-created when the list grows. */
  const totalRef = useRef(visible.length);
  totalRef.current = visible.length;
  /** A row the keyboard asked for that is not mounted yet; focused once it is. */
  const pendingFocus = useRef<string | null>(null);

  /**
   * Re-derive the window from the scroller's geometry.
   *
   * Two rect reads per scroll event: `getBoundingClientRect` is the only way to
   * learn where the list starts inside a scroller it does not own, and the
   * subtraction (`list top - scroller top + scrollTop`) is that position in the
   * scroller's own scroll coordinates, which is what a row index translates to.
   */
  const measure = useCallback((): void => {
    const list = container.current;
    const parent = list ? findScrollParent(list) : null;
    scroller.current = parent;
    if (!list || !parent) {
      setRowWindow((current) => (current === null ? current : null));
      return;
    }
    const height = parent.clientHeight;
    if (height <= 0) {
      setRowWindow((current) => (current === null ? current : null));
      return;
    }
    // Read the row height from the stylesheet on every measure: `pointer: coarse`
    // changes it under a mounted tree, and the arithmetic below has to use the
    // rows the browser actually laid out (WB-04).
    const px = rowHeight(list);
    setRowPx(px);
    const listTop = list.getBoundingClientRect().top - parent.getBoundingClientRect().top
      + parent.scrollTop;
    const first = Math.max(0, Math.floor((parent.scrollTop - listTop) / px));
    const rowsOnScreen = Math.ceil(height / px) + 1;
    const start = Math.max(0, first - OVERSCAN);
    const end = Math.min(totalRef.current, first + rowsOnScreen + OVERSCAN);
    setRowWindow((current) =>
      current && current.start === start && current.end === end ? current : { start, end });
  }, []);

  // Measured once the rows are in the DOM, then on every scroll and resize.
  // A container that has no height yet (a collapsed inspector, a layout-less
  // test) leaves `rowWindow` null and the tree renders every row.
  useLayoutEffect(() => {
    measure();
    const parent = scroller.current;
    if (!parent) return;
    parent.addEventListener('scroll', measure, { passive: true });
    window.addEventListener('resize', measure);
    return () => {
      parent.removeEventListener('scroll', measure);
      window.removeEventListener('resize', measure);
    };
  }, [measure]);

  useEffect(() => {
    if (focusedPath !== null && !visible.some((node) => node.entry.path === focusedPath)) {
      setFocusedPath(null);
    }
  }, [focusedPath, visible]);

  const rowElement = (path: string): HTMLElement | null =>
    Array.from(container.current?.querySelectorAll<HTMLElement>('[role="treeitem"]') ?? [])
      .find((item) => item.dataset.treePath === path) ?? null;

  /** Scroll the scroller so the row at `index` is inside the window, then
   *  re-measure: the focused row has to exist in the DOM before it can take
   *  focus, so a windowed tree cannot focus a row it has not mounted. */
  const revealRow = (index: number): void => {
    const list = container.current;
    const parent = scroller.current;
    if (!list || !parent) return;
    const px = rowHeight(list);
    const listTop = list.getBoundingClientRect().top - parent.getBoundingClientRect().top
      + parent.scrollTop;
    const top = listTop + index * px;
    const bottom = top + px;
    const viewTop = parent.scrollTop;
    const viewBottom = viewTop + parent.clientHeight;
    if (top < viewTop) parent.scrollTop = Math.max(0, top - px);
    else if (bottom > viewBottom) parent.scrollTop = bottom - parent.clientHeight + px;
    else return;
    // The browser also fires `scroll` here, but not synchronously and not in a
    // layout-less test: measuring now is what makes the row exist by the time
    // this keypress's render has committed.
    measure();
  };

  const focusNode = (path: string | null): void => {
    setFocusedPath(path);
    pendingFocus.current = path;
    if (!path) return;
    const mounted = rowElement(path);
    if (mounted) {
      pendingFocus.current = null;
      mounted.focus();
      return;
    }
    const index = visible.findIndex((node) => node.entry.path === path);
    if (index >= 0) revealRow(index);
  };

  // A row that was windowed out when the keyboard asked for it is focused as
  // soon as the window includes it. Cheap enough to re-check on every commit:
  // it returns immediately unless a focus is pending.
  useLayoutEffect(() => {
    const path = pendingFocus.current;
    if (path === null) return;
    const element = rowElement(path);
    if (!element) return;
    pendingFocus.current = null;
    element.focus();
  });

  const currentIndex = Math.max(
    0,
    visible.findIndex((node) => node.entry.path === focusedPath),
  );

  const activate = (entry: WorkspaceEntry): void => {
    if (entry.kind === 'dir') onToggleDir(entry);
    else onOpenFile(entry.path);
  };

  // The helpers are shared; which row they act on is the menu's own `menuNode`.
  const sharedMenuProps = contextMenuProps(menu);
  const armMenu = useCallback((node: FlatNode): void => {
    if (armedPath.current === node.entry.path) return;
    armedPath.current = node.entry.path;
    setMenuNode(node);
  }, []);

  const menuPropsFor = (node: FlatNode): RowMenuProps => ({
    onContextMenu: (event: MouseEvent): void => {
      armMenu(node);
      sharedMenuProps.onContextMenu(event);
    },
    onPointerDown: (event: PointerEvent): void => {
      // A finger that rests opens this row's menu 450ms later, inside the shared
      // helpers, where the node is no longer known — so arm it now. A mouse
      // press is a select-drag, never a menu gesture.
      if (event.pointerType !== 'mouse') armMenu(node);
      sharedMenuProps.onPointerDown(event);
    },
    onPointerMove: sharedMenuProps.onPointerMove,
    onPointerUp: sharedMenuProps.onPointerUp,
    onPointerCancel: sharedMenuProps.onPointerCancel,
  });

  const menuEntry = menuNode?.entry ?? null;
  const menuIsDir = menuEntry?.kind === 'dir';
  const menuAbsolute = menuEntry ? absolutePath(rootPath, menuEntry.path) : null;
  // One form or two. When the absolute path is not derivable the two items would
  // copy the same string, so the second is dropped rather than duplicated.
  const menuTwoForms = menuEntry !== null && menuAbsolute !== null && menuAbsolute !== menuEntry.path;

  const copyMenuPath = async (): Promise<void> => {
    if (!menuEntry) return;
    const copied = await copyText(menuAbsolute ?? menuEntry.path);
    store.pushToast(copied ? 'Path copied.' : 'Could not copy the path.',
      copied ? 'success' : 'error');
  };

  const copyMenuRelativePath = async (): Promise<void> => {
    if (!menuEntry) return;
    const copied = await copyText(menuEntry.path);
    store.pushToast(copied ? 'Relative path copied.' : 'Could not copy the relative path.',
      copied ? 'success' : 'error');
  };

  if (rootEntries.length === 0) {
    return (
      <p class="muted small tree-empty">
        No files are visible at the workspace root. Ignored directories may be hidden.
      </p>
    );
  }

  // Windowed only while the scroller has a height, and only past the threshold
  // where mounting everything starts to cost more than the window does. The
  // range is clamped here rather than in `measure`, because the list can shrink
  // (a directory collapsed) between measurements.
  const windowed = rowWindow !== null && visible.length > WINDOW_THRESHOLD;
  const start = windowed ? Math.min(rowWindow.start, Math.max(0, visible.length - 1)) : 0;
  const end = windowed ? Math.min(Math.max(rowWindow.end, start + 1), visible.length) : visible.length;
  const shown = windowed ? visible.slice(start, end) : visible;

  return (
    <>
      <ul
        class="tree"
        role="tree"
        aria-label={`Workspace files in ${rootPath}`}
        ref={container}
        onKeyDown={(event) => {
          const node = visible[currentIndex];
          if (!node) return;
          if (event.key === 'ArrowDown') {
            event.preventDefault();
            const next = visible[Math.min(visible.length - 1, currentIndex + 1)];
            if (next) focusNode(next.entry.path);
          } else if (event.key === 'ArrowUp') {
            event.preventDefault();
            const previous = visible[Math.max(0, currentIndex - 1)];
            if (previous) focusNode(previous.entry.path);
          } else if (event.key === 'Home') {
            event.preventDefault();
            const first = visible[0];
            if (first) focusNode(first.entry.path);
          } else if (event.key === 'End') {
            event.preventDefault();
            const last = visible[visible.length - 1];
            if (last) focusNode(last.entry.path);
          } else if (event.key === 'ArrowRight') {
            event.preventDefault();
            if (node.entry.kind === 'dir') {
              if (!expanded.has(node.entry.path)) onToggleDir(node.entry);
              else {
                const child = visible[currentIndex + 1];
                if (child && child.depth > node.depth) focusNode(child.entry.path);
              }
            }
          } else if (event.key === 'ArrowLeft') {
            event.preventDefault();
            if (node.entry.kind === 'dir' && expanded.has(node.entry.path)) {
              onToggleDir(node.entry);
              return;
            }
            if (node.depth === 0) return;
            const parentPath = node.entry.path.includes('/')
              ? node.entry.path.slice(0, node.entry.path.lastIndexOf('/'))
              : '.';
            for (let index = currentIndex - 1; index >= 0; index -= 1) {
              const candidate = visible[index];
              if (candidate && candidate.entry.path === parentPath) {
                focusNode(candidate.entry.path);
                break;
              }
            }
          } else if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            activate(node.entry);
          }
        }}
      >
        {/* Spacer rows keep the scrollbar honest: a windowed tree must occupy the
            height of the whole list, or scrolling would end early. `presentation`
            keeps them out of the accessibility tree — they are geometry, not
            content. */}
        {start > 0 ? (
          <li
            role="presentation"
            aria-hidden="true"
            class="tree-spacer"
            style={{ height: `${start * rowPx}px` }}
          />
        ) : null}
        {shown.map((node, offset) => {
          const index = start + offset;
          const isDir = node.entry.kind === 'dir';
          return (
            <TreeRow
              key={`${node.entry.kind}:${node.entry.path}`}
              node={node}
              index={index}
              total={visible.length}
              expanded={isDir && expanded.has(node.entry.path)}
              loading={isDir && loading.has(node.entry.path)}
              tabbable={focusedPath === null ? index === 0 : focusedPath === node.entry.path}
              selected={focusedPath === node.entry.path}
              menuProps={menuPropsFor(node)}
              onActivate={activate}
              onFocusRow={(path) => {
                pendingFocus.current = null;
                setFocusedPath(path);
              }}
            />
          );
        })}
        {end < visible.length ? (
          <li
            role="presentation"
            aria-hidden="true"
            class="tree-spacer"
            style={{ height: `${(visible.length - end) * rowPx}px` }}
          />
        ) : null}
      </ul>
      {/* One menu for the tree, portalled onto the menu layer (`float`) where no
          scroller in the inspector can clip it. Mounted outside the `ul` so the
          tree's children stay `treeitem`s and spacers. */}
      {menuEntry ? (
        <Menu menu={menu} float label={`${menuIsDir ? 'Folder' : 'File'} actions for ${menuEntry.name}`}>
          {/* A directory has no **Open**: expanding one is the row's own click and
            * Enter, and "Open" would be a second word for the same thing. */}
          {menuIsDir ? null : (
            <MenuItem menu={menu} action={{ label: 'Open', onSelect: () => activate(menuEntry) }} />
          )}
          {menuIsDir || !onOpenFileInTab ? null : (
            <MenuItem
              menu={menu}
              action={{ label: 'Open in a tab', onSelect: () => onOpenFileInTab?.(menuEntry.path) }}
            />
          )}
          <MenuItem
            menu={menu}
            action={{
              label: 'Copy path',
              title: menuAbsolute ?? menuEntry.path,
              onSelect: () => void copyMenuPath(),
            }}
          />
          {menuTwoForms ? (
            <MenuItem
              menu={menu}
              action={{
                label: 'Copy relative path',
                title: menuEntry.path,
                onSelect: () => void copyMenuRelativePath(),
              }}
            />
          ) : null}
        </Menu>
      ) : null}
    </>
  );
}
