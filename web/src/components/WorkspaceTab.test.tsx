import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/preact';
import * as api from '../api/endpoints';
import * as store from '../state/store';
import { configFixture, resetAppState } from '../test/fixtures';
import { WorkspaceTab } from './WorkspaceTab';
import type { WorkspaceEntry, WorkspaceTree } from '../api/types';

vi.mock('../api/endpoints', () => ({
  getWorkspaceTree: vi.fn(),
  getWorkspaceFile: vi.fn(),
}));

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
