import type { JSX } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import type { WorkspaceEntry } from '../api/types';
import { ArrowRightIcon, ChevronRightIcon, FileCodeIcon, FileIcon, FolderIcon, FolderOpenIcon } from './icons';

export interface FileTreeProps {
  rootPath: string;
  rootEntries: WorkspaceEntry[];
  children: Record<string, WorkspaceEntry[]>;
  expanded: Set<string>;
  loading: Set<string>;
  onToggleDir: (entry: WorkspaceEntry) => void;
  onOpenFile: (path: string) => void;
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

/** Keyboard-operable tree (roving tabindex; Arrow keys, Home/End, Enter/Space). */
export function FileTree({
  rootPath,
  rootEntries,
  children,
  expanded,
  loading,
  onToggleDir,
  onOpenFile,
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
        const isOpen = isDir && expanded.has(node.entry.path);
        const isLoading = isDir && loading.has(node.entry.path);
        const tabbable = focusedPath === null ? node === visible[0] : focusedPath === node.entry.path;
        const selected = focusedPath === node.entry.path;
        const kind: FileKind = isDir ? 'folder' : fileKind(node.entry.name);
        return (
          <li
            key={`${node.entry.kind}:${node.entry.path}`}
            role="treeitem"
            aria-level={node.depth + 1}
            aria-expanded={isDir ? isOpen : undefined}
            aria-selected={selected}
            data-tree-path={node.entry.path}
            tabIndex={tabbable ? 0 : -1}
            class={`tree-row${isDir ? ' is-dir' : ' is-file'}${isOpen ? ' is-open' : ''}${
              selected ? ' is-selected' : ''
            }`}
            style={{ paddingLeft: `${node.depth * 18 + 8}px` }}
            title={node.entry.path}
            onClick={() => {
              setFocusedPath(node.entry.path);
              activate(node);
            }}
            onFocus={() => setFocusedPath(node.entry.path)}
          >
            <span class={`tree-chevron${isOpen ? ' is-open' : ''}`} aria-hidden="true">
              {isDir ? <ChevronRightIcon size={12} /> : null}
            </span>
            <span class={`tree-icon kind-${kind}`} aria-hidden="true">
              {iconFor(kind, isOpen)}
            </span>
            <span class="tree-name">{node.entry.name}</span>
            {isLoading ? <span class="tree-loading">loading…</span> : null}
            {selected ? (
              <span class="tree-open-hint" aria-hidden="true">
                <ArrowRightIcon size={12} />
              </span>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}