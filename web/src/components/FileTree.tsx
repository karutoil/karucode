import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
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

interface TreeRowProps {
  node: FlatNode;
  /** The workspace root the listing is relative to, as `FileTree` received it. */
  rootPath: string;
  expanded: boolean;
  loading: boolean;
  tabbable: boolean;
  selected: boolean;
  onActivate: (node: FlatNode) => void;
  onFocusRow: (path: string) => void;
  onOpenInTab?: (path: string) => void;
}

/**
 * One tree row: the `treeitem` and its right-click menu.
 *
 * This is its own component because a menu is per row — it opens at *this* row's
 * pointer and returns focus to *this* row, so one controller cannot serve the
 * list. The row has no kebab (a tree of hundreds of rows would be hundreds of
 * buttons), so the menu is pointer-anchored only: `float`, portalled onto the
 * menu layer where no scroller in the inspector can clip it.
 *
 * Nothing here is the only path to an action. A file still opens on click and on
 * Enter/Space, a directory still expands the same way, and the path is in the
 * row's `title`. The menu adds the two things the row never had room for: opening
 * without moving the pointer to it, and copying a path into a terminal command.
 */
function TreeRow({
  node,
  rootPath,
  expanded,
  loading,
  tabbable,
  selected,
  onActivate,
  onFocusRow,
  onOpenInTab,
}: TreeRowProps): JSX.Element {
  const { entry, depth } = node;
  const isDir = entry.kind === 'dir';
  const kind: FileKind = isDir ? 'folder' : fileKind(entry.name);
  const menu = useMenu();
  // Focus returns to the row itself: it is the focusable `treeitem` a keyboard
  // user would have activated, and this menu has no kebab to return to.
  const rowRef = useRef<HTMLLIElement>(null);
  const absolute = absolutePath(rootPath, entry.path);
  // One form or two. When the absolute path is not derivable the two items would
  // copy the same string, so the second is dropped rather than duplicated.
  const twoForms = absolute !== null && absolute !== entry.path;

  const copyPath = async (): Promise<void> => {
    const copied = await copyText(absolute ?? entry.path);
    store.pushToast(copied ? 'Path copied.' : 'Could not copy the path.',
      copied ? 'success' : 'error');
  };

  const copyRelativePath = async (): Promise<void> => {
    const copied = await copyText(entry.path);
    store.pushToast(copied ? 'Relative path copied.' : 'Could not copy the relative path.',
      copied ? 'success' : 'error');
  };

  return (
    <li
      ref={rowRef}
      role="treeitem"
      aria-level={depth + 1}
      aria-expanded={isDir ? expanded : undefined}
      aria-selected={selected}
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
        onActivate(node);
      }}
      onFocus={() => onFocusRow(entry.path)}
      {...contextMenuProps(menu, undefined, () => rowRef.current)}
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
      <Menu menu={menu} float label={`${isDir ? 'Folder' : 'File'} actions for ${entry.name}`}>
        {/* A directory has no **Open**: expanding one is the row's own click and
          * Enter, and "Open" would be a second word for the same thing. */}
        {isDir ? null : (
          <MenuItem menu={menu} action={{ label: 'Open', onSelect: () => onActivate(node) }} />
        )}
        {isDir || !onOpenInTab ? null : (
          <MenuItem
            menu={menu}
            action={{ label: 'Open in a tab', onSelect: () => onOpenInTab(entry.path) }}
          />
        )}
        <MenuItem
          menu={menu}
          action={{
            label: 'Copy path',
            title: absolute ?? entry.path,
            onSelect: () => void copyPath(),
          }}
        />
        {twoForms ? (
          <MenuItem
            menu={menu}
            action={{
              label: 'Copy relative path',
              title: entry.path,
              onSelect: () => void copyRelativePath(),
            }}
          />
        ) : null}
      </Menu>
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

  const visible = useMemo(
    () => flatten(rootEntries, 0, children, expanded),
    [rootEntries, children, expanded],
  );

  useEffect(() => {
    if (focusedPath !== null && !visible.some((node) => node.entry.path === focusedPath)) {
      setFocusedPath(null);
    }
  }, [focusedPath, visible]);

  const focusNode = (path: string | null): void => {
    setFocusedPath(path);
    if (!path) return;
    const element = Array.from(
      container.current?.querySelectorAll<HTMLElement>('[role="treeitem"]') ?? [],
    ).find((item) => item.dataset.treePath === path);
    element?.focus();
  };

  const currentIndex = Math.max(
    0,
    visible.findIndex((node) => node.entry.path === focusedPath),
  );

  const activate = (node: FlatNode): void => {
    if (node.entry.kind === 'dir') onToggleDir(node.entry);
    else onOpenFile(node.entry.path);
  };

  if (rootEntries.length === 0) {
    return (
      <p class="muted small tree-empty">
        No files are visible at the workspace root. Ignored directories may be hidden.
      </p>
    );
  }

  return (
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
          const parent = [...visible]
            .reverse()
            .find((candidate) => candidate.entry.path === parentPath);
          if (parent) focusNode(parent.entry.path);
        } else if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          activate(node);
        }
      }}
    >
      {visible.map((node) => {
        const isDir = node.entry.kind === 'dir';
        return (
          <TreeRow
            key={`${node.entry.kind}:${node.entry.path}`}
            node={node}
            rootPath={rootPath}
            expanded={isDir && expanded.has(node.entry.path)}
            loading={isDir && loading.has(node.entry.path)}
            tabbable={focusedPath === null ? node === visible[0] : focusedPath === node.entry.path}
            selected={focusedPath === node.entry.path}
            onActivate={activate}
            onFocusRow={setFocusedPath}
            onOpenInTab={onOpenFileInTab}
          />
        );
      })}
    </ul>
  );
}