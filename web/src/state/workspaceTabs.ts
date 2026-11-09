import { signal } from '@preact/signals';

/**
 * Which centre-pane view is selected, and which files are open as tabs.
 *
 * The three built-in views (Conversation, Source control, Terminal) live in the
 * same strip as one tab per open file, so "open this file in the workspace" is
 * one more tab rather than a second pane: the reader keeps the conversation, the
 * shell and as many files as they need side by side, and closes each when the
 * question it answered is over.
 *
 * It is module state rather than `ChatView`'s own `useState` because the choice
 * is made from *outside* the component tree — the file viewer's toolbar, the
 * explorer's context menu, a rendered link inside a Markdown preview — and those
 * callers have no way to reach into a component's local state. `ChatView` reads
 * the signal, so selection, closing and the strip stay consistent no matter who
 * asked.
 */
export type WorkspaceView = 'chat' | 'changes' | 'terminal' | `file:${string}`;

/** Workspace-relative paths of the open file tabs, in the order they were opened. */
export const openFileTabs = signal<readonly string[]>([]);

export const workspaceView = signal<WorkspaceView>('chat');

const FILE_PREFIX = 'file:';

export function fileView(path: string): WorkspaceView {
  // Encoded because the id becomes a DOM id (`workspace-tab-file:…`): a path may
  // contain a space, `#` or `%`, none of which survive an unescaped id.
  return `${FILE_PREFIX}${encodeURIComponent(path)}`;
}

export function isFileView(view: WorkspaceView): boolean {
  return view.startsWith(FILE_PREFIX);
}

/** The path a `file:` view stands for, or `null` for a built-in view. */
export function fileViewPath(view: WorkspaceView): string | null {
  if (!isFileView(view)) return null;
  const encoded = view.slice(FILE_PREFIX.length);
  try {
    return decodeURIComponent(encoded);
  } catch {
    // A malformed escape is taken literally rather than dropped.
    return encoded;
  }
}

/** Show a file in the centre pane, reusing its tab when it is already open. */
export function openFileTab(path: string): void {
  if (!openFileTabs.value.includes(path)) {
    openFileTabs.value = [...openFileTabs.value, path];
  }
  workspaceView.value = fileView(path);
}

export function selectWorkspaceView(view: WorkspaceView): void {
  workspaceView.value = view;
}

/** Close one file tab, selecting a neighbour when the closed one was visible. */
export function closeFileTab(path: string): void {
  const tabs = openFileTabs.value;
  const index = tabs.indexOf(path);
  if (index === -1) return;
  const remaining = tabs.filter((item) => item !== path);
  openFileTabs.value = remaining;
  if (workspaceView.value !== fileView(path)) return;
  // The tab that takes the closed one's place, else the one before it: closing
  // must never leave the strip pointing at a view that is no longer there.
  const neighbour = remaining[index] ?? remaining[index - 1] ?? null;
  workspaceView.value = neighbour === null ? 'chat' : fileView(neighbour);
}

export function closeAllFileTabs(): void {
  openFileTabs.value = [];
  if (isFileView(workspaceView.value)) workspaceView.value = 'chat';
}

/**
 * A tab's own "may I close?" check.
 *
 * A file panel can hold unsaved text, and the close button that ends it lives in
 * the tab strip — outside the component that owns that text. A mounted panel
 * therefore registers a guard that answers `true` when it handled the request
 * itself (by asking), so `requestCloseFileTab` never throws a draft away behind
 * the editor's back.
 */
type CloseGuard = () => boolean;

const closeGuards = new Map<string, CloseGuard>();

/** Register a panel's close guard; the returned function unregisters it. */
export function registerFileTabCloseGuard(path: string, guard: CloseGuard): () => void {
  closeGuards.set(path, guard);
  return () => {
    if (closeGuards.get(path) === guard) closeGuards.delete(path);
  };
}

/** Close a file tab the way its own panel would: asking first when it must. */
export function requestCloseFileTab(path: string): void {
  if (closeGuards.get(path)?.() === true) return;
  closeFileTab(path);
}

/** Test seam: forget every tab, guard and selection. */
export function resetWorkspaceTabs(): void {
  closeGuards.clear();
  openFileTabs.value = [];
  workspaceView.value = 'chat';
}
