import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import * as api from '../api/endpoints';
import * as store from '../state/store';
import { copyText } from '../lib/clipboard';
import { configFixture, resetAppState } from '../test/fixtures';
import { WorkspaceTab } from './WorkspaceTab';
import type { WorkspaceEntry, WorkspaceTree } from '../api/types';

vi.mock('../api/endpoints', () => ({
  getWorkspaceTree: vi.fn(),
  getWorkspaceFile: vi.fn(),
}));

// jsdom has no Clipboard API and no working `execCommand`, so `lib/clipboard`
// resolves false there. Mocked so these tests assert *what* was copied rather
// than whether the browser allowed it (the same choice `GitManager.test.tsx` makes).
vi.mock('../lib/clipboard', () => ({ copyText: vi.fn() }));

const copyTextMock = vi.mocked(copyText);

function entry(path: string, kind = 'file'): WorkspaceEntry {
  return { name: path.split('/').pop() ?? path, path, kind, size: 1 };
}
function tree(entries: WorkspaceEntry[], truncated = false): WorkspaceTree {
  return { path: '.', entries, truncated, limit: 2000 };
}

beforeEach(() => {
  resetAppState();
  vi.clearAllMocks();
});

describe('Workspace explorer truncation', () => {
  it('shows that the root is partial instead of implying the listing is complete', async () => {
    vi.mocked(api.getWorkspaceTree).mockResolvedValue(tree([], true));
    render(<WorkspaceTab />);
    await screen.findByText('No files are visible at the workspace root. Ignored directories may be hidden.');
    const warning = screen.getByRole('status');
    expect(warning.classList.contains('tree-partial-warning')).toBe(true);
    expect(warning.textContent).toContain('partially listed');
    expect(warning.textContent).toContain('file search');
  });

  it('retains a partial indicator for a lazily fetched child', async () => {
    vi.mocked(api.getWorkspaceTree).mockImplementation(async (path) =>
      path === '.' ? tree([entry('src', 'dir')]) : tree([entry('src/nested.rs')], true),
    );
    render(<WorkspaceTab />);
    fireEvent.click(await screen.findByRole('treeitem', { name: 'src' }));
    expect(await screen.findByText('nested.rs')).toBeTruthy();
    expect(screen.getByRole('status').textContent).toContain('Some folders are only partially listed');
  });

  it('reloads the same quick-preview path with project B real bytes after project A', async () => {
    vi.mocked(api.getWorkspaceTree).mockImplementation(async () => tree([entry('same.md')]));
    vi.mocked(api.getWorkspaceFile).mockImplementation(async (path, project) => {
      const content = project === 'p_b' ? 'bytes from project B' : 'bytes from project A';
      return {
        path,
        content,
        size: new TextEncoder().encode(content).length,
        digest: `sha256:${project === 'p_b' ? 'b' : 'a'}`.padEnd(71, '0'),
        truncated: false,
        binary: false,
        language: 'markdown',
      };
    });
    store.currentProjectId.value = 'p_a';
    const { container } = render(<WorkspaceTab />);
    fireEvent.click(await screen.findByRole('treeitem', { name: 'same.md' }));
    await waitFor(() => expect(container.querySelector('.preview-code')?.textContent).toContain('bytes from project A'));

    await act(async () => { store.currentProjectId.value = 'p_b'; });
    await waitFor(() => expect(container.querySelector('.preview-code')?.textContent).toContain('bytes from project B'));
    expect(container.querySelector('.preview-code')?.textContent).not.toContain('bytes from project A');
    expect(vi.mocked(api.getWorkspaceFile).mock.calls.map(([, project]) => project)).toEqual(['p_a', 'p_b']);
  });

  it('uses the selected project’s canonical path rather than the stale global default', async () => {
    const mockTree = vi.mocked(api.getWorkspaceTree).mockResolvedValue(tree([]));
    store.projects.value = ['p_a', 'p_b'].map((id) => ({
      id,
      name: id,
      path: id === 'p_a' ? '/tmp/workspace-a' : '/tmp/workspace-b',
      created_at: 0,
      updated_at: 0,
      session_count: 0,
    }));
    store.config.value = { ...configFixture, workspace: '/stale-global-default' };
    store.currentProjectId.value = 'p_a';
    const { container } = render(<WorkspaceTab />);
    await waitFor(() => expect(mockTree).toHaveBeenCalled());
    expect(container.querySelector('.workspace-path')?.getAttribute('title')).toBe('/tmp/workspace-a');

    await act(async () => { store.currentProjectId.value = 'p_b'; });
    await waitFor(() =>
      expect(container.querySelector('.workspace-path')?.getAttribute('title')).toBe('/tmp/workspace-b'),
    );
    expect(container.querySelector('.workspace-path')?.textContent).toContain('/tmp/workspace-b');
    expect(container.querySelector('.workspace-path')?.textContent).not.toContain('/stale-global-default');
  });

  it('does not resurrect A children or their partial marker after switching to B', async () => {
    let child!: (value: WorkspaceTree) => void;
    const childPending = new Promise<WorkspaceTree>((resolve) => { child = resolve; });
    vi.mocked(api.getWorkspaceTree).mockImplementation((path, _depth, project) => {
      if (path !== '.') return childPending;
      return Promise.resolve(project === 'p_b' ? tree([entry('from-b.txt')]) : tree([entry('src', 'dir')]));
    });
    store.currentProjectId.value = 'p_a';
    const { container } = render(<WorkspaceTab />);
    fireEvent.click(await screen.findByRole('treeitem', { name: 'src' }));
    await waitFor(() => expect(api.getWorkspaceTree).toHaveBeenCalledWith('src', 1, 'p_a'));

    store.currentProjectId.value = 'p_b';
    expect(await screen.findByText('from-b.txt')).toBeTruthy();
    await act(async () => { child(tree([entry('src/from-a.txt')], true)); });
    expect(screen.queryByText('from-a.txt')).toBeNull();
    expect(container.querySelector('.tree-partial-warning')).toBeNull();
  });
});

describe('Workspace tree row context menu', () => {
  /**
   * Render the explorer over a file, a directory and one nested file behind it.
   *
   * `projectPath` is what becomes the tree's `rootPath`: `WorkspaceTab` resolves
   * it from the selected project, then health, then config, then `'.'`. Pass
   * `null` for that last case — nothing registered, no workspace known — where
   * the tree has no absolute path to join onto and the menu must say so by
   * dropping an item rather than inventing one.
   */
  async function renderTree(projectPath: string | null = '/home/me/project'): Promise<void> {
    vi.mocked(api.getWorkspaceTree).mockImplementation(async (path) =>
      path === '.'
        ? tree([entry('README.md'), entry('src', 'dir')])
        : tree([entry('src/main.rs')]),
    );
    vi.mocked(api.getWorkspaceFile).mockImplementation(async (file) => ({
      path: file,
      content: 'fn main() {}\n',
      size: 14,
      digest: `sha256:${'0'.repeat(64)}`,
      truncated: false,
      binary: false,
      language: 'rust',
    }));
    store.projects.value = projectPath === null ? [] : [{
      id: 'p_root',
      name: 'karucode',
      path: projectPath,
      created_at: 0,
      updated_at: 0,
      session_count: 0,
    }];
    store.currentProjectId.value = projectPath === null ? null : 'p_root';
    render(<WorkspaceTab />);
    await screen.findByRole('treeitem', { name: 'README.md' });
  }

  function row(name: string): HTMLElement {
    return screen.getByRole('treeitem', { name });
  }

  /** Expand `src` and return its nested row: the path the menu must join. */
  async function nestedRow(): Promise<HTMLElement> {
    fireEvent.click(row('src'));
    return screen.findByRole('treeitem', { name: 'main.rs' });
  }

  it('opens on right-click with the file’s actions, and Escape returns focus to the row', async () => {
    await renderTree();
    const fileRow = row('README.md');

    // `fireEvent` returns false when the event was cancelled: our menu replaces
    // the browser's, so the browser's must not also appear.
    expect(fireEvent.contextMenu(fileRow, { clientX: 40, clientY: 60 })).toBe(false);

    const menu = screen.getByRole('menu', { name: 'File actions for README.md' });
    // A file offers the workspace tab beside the dialog: same document, no modal.
    expect(within(menu).getAllByRole('menuitem').map((item) => item.textContent))
      .toEqual(['Open', 'Open in a tab', 'Copy path', 'Copy relative path']);
    // Opening focuses the first item, and the menu is portalled onto the menu
    // layer, where no scroller in the inspector can clip it.
    expect(document.activeElement).toBe(within(menu).getByRole('menuitem', { name: 'Open' }));
    expect(menu.closest('.menu-layer')?.parentElement).toBe(document.body);

    fireEvent.keyDown(menu, { key: 'Escape' });
    expect(screen.queryByRole('menu')).toBeNull();
    // Focus goes back to the row itself: it is the focusable `treeitem` a keyboard
    // user would have activated, and this row has no kebab to return to.
    expect(document.activeElement).toBe(fileRow);
  });

  it('omits Open for a directory, which the row itself already expands', async () => {
    await renderTree();
    fireEvent.contextMenu(row('src'), { clientX: 40, clientY: 80 });

    const menu = screen.getByRole('menu', { name: 'Folder actions for src' });
    expect(within(menu).queryByRole('menuitem', { name: 'Open' })).toBeNull();
    expect(within(menu).getAllByRole('menuitem').map((item) => item.textContent))
      .toEqual(['Copy path', 'Copy relative path']);
    // Expanding a folder is still the row's own click: the menu never became the
    // only way in.
    fireEvent.keyDown(menu, { key: 'Escape' });
    fireEvent.click(row('src'));
    expect(await screen.findByRole('treeitem', { name: 'main.rs' })).toBeTruthy();
  });

  it('copies the joined absolute path rather than the name the row shows', async () => {
    copyTextMock.mockResolvedValue(true);
    await renderTree();
    const nested = await nestedRow();

    fireEvent.contextMenu(nested, { clientX: 40, clientY: 60 });
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy path' }));

    await waitFor(() => expect(copyTextMock).toHaveBeenCalledWith('/home/me/project/src/main.rs'));
    expect(screen.queryByRole('menu')).toBeNull();
    expect(store.toasts.value.at(-1)).toMatchObject({ message: 'Path copied.', tone: 'success' });
  });

  it('copies the workspace-relative path from its own item', async () => {
    copyTextMock.mockResolvedValue(true);
    await renderTree();

    fireEvent.contextMenu(row('README.md'), { clientX: 40, clientY: 60 });
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy relative path' }));

    await waitFor(() => expect(copyTextMock).toHaveBeenCalledWith('README.md'));
    expect(store.toasts.value.at(-1)).toMatchObject({
      message: 'Relative path copied.',
      tone: 'success',
    });
  });

  it('says so when the clipboard refuses the copy', async () => {
    copyTextMock.mockResolvedValue(false);
    await renderTree();

    fireEvent.contextMenu(row('README.md'), { clientX: 40, clientY: 60 });
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy path' }));

    await waitFor(() =>
      expect(store.toasts.value.at(-1)).toMatchObject({
        message: 'Could not copy the path.',
        tone: 'error',
      }),
    );
  });

  it('drops the second copy item instead of inventing a path when no workspace is known', async () => {
    // No project registered and no health/config workspace, so `WorkspaceTab`
    // hands the tree `'.'` as its root. Joining a relative path onto that is not
    // an absolute path, and a menu that said otherwise would hand the reader a
    // string that resolves against whatever shell they pasted it into.
    await renderTree(null);
    fireEvent.contextMenu(row('README.md'), { clientX: 40, clientY: 60 });

    const menu = screen.getByRole('menu', { name: 'File actions for README.md' });
    expect(within(menu).getAllByRole('menuitem').map((item) => item.textContent))
      .toEqual(['Open', 'Open in a tab', 'Copy path']);

    // The one item that is left copies the row's own path, and still says so.
    copyTextMock.mockResolvedValue(true);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy path' }));
    await waitFor(() => expect(copyTextMock).toHaveBeenCalledWith('README.md'));
    expect(store.toasts.value.at(-1)).toMatchObject({ message: 'Path copied.', tone: 'success' });
  });

  it('joins onto the selected project’s own path, end to end', async () => {
    // The integration half of the join the cases above assert directly: the tree's
    // `rootPath` is the selected project's path, so a wrong root here would copy a
    // wrong path everywhere.
    copyTextMock.mockResolvedValue(true);
    vi.mocked(api.getWorkspaceTree).mockImplementation(async (path) =>
      path === '.' ? tree([entry('README.md'), entry('docs', 'dir')]) : tree([entry('docs/guide.md')]),
    );
    vi.mocked(api.getWorkspaceFile).mockResolvedValue({
      path: 'README.md',
      content: '# hi\n',
      size: 5,
      digest: `sha256:${'0'.repeat(64)}`,
      truncated: false,
      binary: false,
      language: 'markdown',
    });
    store.projects.value = [{
      id: 'p_live',
      name: 'karucode',
      path: '/work/karucode',
      created_at: 0,
      updated_at: 0,
      session_count: 0,
    }];
    store.currentProjectId.value = 'p_live';
    render(<WorkspaceTab />);

    fireEvent.click(await screen.findByRole('treeitem', { name: 'docs' }));
    const nested = await screen.findByRole('treeitem', { name: 'guide.md' });
    fireEvent.contextMenu(nested, { clientX: 40, clientY: 60 });

    const menu = screen.getByRole('menu', { name: 'File actions for guide.md' });
    expect(within(menu).getAllByRole('menuitem').map((item) => item.textContent))
      .toEqual(['Open', 'Open in a tab', 'Copy path', 'Copy relative path']);
    fireEvent.click(within(menu).getByRole('menuitem', { name: 'Copy path' }));
    await waitFor(() => expect(copyTextMock).toHaveBeenCalledWith('/work/karucode/docs/guide.md'));

    fireEvent.contextMenu(screen.getByRole('treeitem', { name: 'guide.md' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy relative path' }));
    await waitFor(() => expect(copyTextMock).toHaveBeenLastCalledWith('docs/guide.md'));
  });

  it('opens the file the row’s own click and Enter open', async () => {
    await renderTree();

    fireEvent.contextMenu(row('README.md'), { clientX: 40, clientY: 60 });
    fireEvent.click(screen.getByRole('menuitem', { name: 'Open' }));

    expect(screen.queryByRole('menu')).toBeNull();
    // The same handler as the row's click and Enter: the viewer takes the path,
    // and the quick preview reads that same file.
    expect(store.fileViewerPath.value).toBe('README.md');
    await waitFor(() =>
      expect(vi.mocked(api.getWorkspaceFile).mock.calls[0]?.[0]).toBe('README.md'),
    );
  });
});
