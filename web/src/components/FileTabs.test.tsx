import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import { ChatView } from './ChatView';
import { FileViewer } from './FileViewer';
import * as store from '../state/store';
import * as terminalState from '../state/terminals';
import * as workspaceTabs from '../state/workspaceTabs';
import { configFixture, errorResponse, jsonResponse, resetAppState } from '../test/fixtures';

/**
 * A file opened as a tab in the workspace strip, exercised through the real
 * `ChatView` + `FileViewer` pair: the modal offers "Open in tab", the strip then
 * owns the document, and closing the tab puts the workspace back where it was.
 */

interface Harness {
  calls: string[];
  fileReads: () => number;
}

function installFetch(files: Record<string, string>): Harness {
  const calls: string[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(String(input), 'http://localhost');
      calls.push(`${url.pathname}${url.search}`);
      if (url.pathname === '/api/workspace/file') {
        const path = url.searchParams.get('path') ?? '';
        const content = files[path];
        if (content === undefined) return errorResponse('not_found', `no ${path}`, 404);
        return jsonResponse({
          path,
          content,
          size: new TextEncoder().encode(content).length,
          digest: `sha256:${'a'.repeat(64)}`,
          truncated: false,
          binary: false,
          language: path.endsWith('.md') ? 'markdown' : 'rust',
        });
      }
      if (url.pathname === '/api/workspace/tree') {
        return jsonResponse({ path: '.', entries: [], truncated: false });
      }
      if (url.pathname === '/api/terminals') {
        return jsonResponse({ enabled: false, terminals: [] });
      }
      return errorResponse('not_found', `unmocked ${url.pathname}`, 404);
    }),
  );
  return {
    calls,
    fileReads: () => calls.filter((call) => call.startsWith('/api/workspace/file')).length,
  };
}

function renderWorkbench(): void {
  store.currentSessionId.value = 's_ab12';
  store.config.value = configFixture;
  store.sessions.value = [
    {
      id: 's_ab12',
      title: 'Fix login bug',
      workspace: '/home/me/project',
      project_id: null,
      created_at: 1,
      updated_at: 2,
      message_count: 0,
      last_run_status: 'completed',
      running: false,
    },
  ];
  render(
    <>
      <ChatView />
      <FileViewer />
    </>,
  );
}

function workspaceTab(name: RegExp): HTMLElement {
  return within(screen.getByRole('tablist', { name: 'Workspace views' })).getByRole('tab', {
    name,
  });
}

describe('file tabs in the workspace strip', () => {
  beforeEach(() => {
    resetAppState();
    terminalState.resetTerminalState();
    Object.defineProperty(URL, 'createObjectURL', {
      configurable: true,
      value: () => 'blob:file-preview',
    });
    Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: () => undefined });
  });

  afterEach(() => {
    resetAppState();
    terminalState.resetTerminalState();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('moves an open dialog into a tab with "Open in tab", and back into the dialog', async () => {
    installFetch({ 'docs/guide.md': '# Guide\n\nbody' });
    renderWorkbench();

    act(() => {
      store.fileViewerPath.value = 'docs/guide.md';
    });
    const dialog = await screen.findByRole('dialog');
    await waitFor(() => expect(within(dialog).getByTestId('file-preview-markdown')).toBeTruthy());

    fireEvent.click(within(dialog).getByRole('button', { name: /open in tab/i }));

    // The dialog gave the document to the strip rather than duplicating it.
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(store.fileViewerPath.value).toBeNull();
    const tab = workspaceTab(/guide\.md/);
    expect(tab.getAttribute('aria-selected')).toBe('true');
    expect(tab.getAttribute('title')).toBe('docs/guide.md');
    // The panel really renders the file, in the centre pane, with its path in the
    // status bar (the strip's own tab names it, so the panel has no header of its
    // own for it).
    const panel = document.getElementById('workspace-panel-file:docs%2Fguide.md');
    expect(panel).not.toBeNull();
    expect(panel?.hidden).toBe(false);
    expect(panel?.querySelector('.file-status-path')?.textContent).toBe('docs/guide.md');
    await waitFor(() =>
      expect(panel?.querySelector('.markdown h1')?.textContent).toBe('Guide'),
    );

    // "Open in dialog" hands it back: the dialog shows the same file again.
    fireEvent.click(screen.getByRole('button', { name: /open in dialog/i }));
    const reopened = await screen.findByRole('dialog');
    await waitFor(() => expect(within(reopened).getByTestId('file-preview-markdown')).toBeTruthy());
    expect(within(reopened).getByRole('heading', { name: 'Guide' })).toBeTruthy();
  });

  it('puts one quiet bar over a file tab instead of a stacked grey header', async () => {
    // What this replaced: a tab stacked a path header (`.file-view-head`, with a
    // second close button), a metadata toolbar (language badge, size, three
    // bordered chips) and a Preview/Source band - about 140px of grey above a
    // document whose name the strip's own tab and the session breadcrumb already
    // carry. One bar now holds the view toggle and the actions, and the file's
    // own facts live in the status bar at the bottom.
    installFetch({ 'docs/guide.md': '# Guide\n\nbody' });
    renderWorkbench();

    act(() => workspaceTabs.openFileTab('docs/guide.md'));
    const panel = await waitFor(() => {
      const element = document.getElementById('workspace-panel-file:docs%2Fguide.md');
      expect(element).not.toBeNull();
      return element as HTMLElement;
    });
    await waitFor(() => expect(panel.querySelector('.markdown h1')?.textContent).toBe('Guide'));

    // No path header, and no second band under the toolbar.
    expect(panel.querySelector('.file-view-head')).toBeNull();
    expect(panel.querySelector('.file-tabbar')).toBeNull();
    expect(panel.querySelector('.file-view-path')).toBeNull();
    const bars = panel.querySelectorAll('.file-toolbar');
    expect(bars).toHaveLength(1);
    const bar = bars[0] as HTMLElement;
    // The one bar is the view toggle plus the actions.
    expect(bar.querySelector('.file-view-tabs')).not.toBeNull();
    expect(within(bar).getByRole('button', { name: /open in dialog/i })).toBeTruthy();
    expect(within(bar).getByRole('button', { name: /copy path/i })).toBeTruthy();
    expect(within(bar).getByRole('button', { name: /^edit$/i })).toBeTruthy();
    // Closing belongs to the strip's tab, not to a second button inside the panel.
    expect(within(panel).queryByRole('button', { name: /^close /i })).toBeNull();

    // The facts the header used to repeat are stated once, in the status bar.
    const status = panel.querySelector('.file-statusbar')?.textContent ?? '';
    expect(status).toContain('docs/guide.md');
    expect(status).toContain('markdown');
    expect(status).toContain('13 B');
    expect(status).toContain('3 lines');
  });

  it('keeps a file tab mounted - and does not refetch it - while the reader visits the conversation', async () => {
    const harness = installFetch({ 'docs/guide.md': '# Guide\n\nbody' });
    renderWorkbench();

    act(() => workspaceTabs.openFileTab('docs/guide.md'));
    await waitFor(() =>
      expect(document.querySelector('#workspace-panel-file\\:docs\\%2Fguide\\.md h1')).not.toBeNull(),
    );
    const reads = harness.fileReads();
    expect(reads).toBe(1);

    fireEvent.click(workspaceTab(/^Conversation/));
    expect(workspaceTab(/guide\.md/).getAttribute('aria-selected')).toBe('false');
    expect(
      document.getElementById('workspace-panel-file:docs%2Fguide.md')?.hidden,
    ).toBe(true);

    fireEvent.click(workspaceTab(/guide\.md/));
    expect(workspaceTab(/guide\.md/).getAttribute('aria-selected')).toBe('true');
    // Still the same mounted document: no second read, and the content is there.
    expect(harness.fileReads()).toBe(reads);
    expect(
      document.getElementById('workspace-panel-file:docs%2Fguide.md')?.hidden,
    ).toBe(false);
  });

  it('closes the tab from the strip and returns to the conversation', async () => {
    installFetch({ 'docs/guide.md': '# Guide\n\nbody' });
    renderWorkbench();

    act(() => workspaceTabs.openFileTab('docs/guide.md'));
    await waitFor(() => expect(screen.getByRole('tab', { name: /guide\.md/ })).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: /close guide\.md/i }));

    expect(workspaceTabs.openFileTabs.value).toEqual([]);
    expect(screen.queryByRole('tab', { name: /guide\.md/ })).toBeNull();
    expect(document.getElementById('workspace-panel-file:docs%2Fguide.md')).toBeNull();
    expect(workspaceTab(/^Conversation/).getAttribute('aria-selected')).toBe('true');
  });

  it('asks before a tab holding unsaved edits is closed', async () => {
    installFetch({ 'docs/guide.md': '# Guide\n\nbody' });
    renderWorkbench();

    act(() => workspaceTabs.openFileTab('docs/guide.md'));
    const panel = await waitFor(() => {
      const element = document.getElementById('workspace-panel-file:docs%2Fguide.md');
      expect(element).not.toBeNull();
      return element as HTMLElement;
    });
    fireEvent.click(within(panel).getByRole('button', { name: /edit/i }));
    await waitFor(() => expect(panel.querySelector('textarea')).not.toBeNull());
    const editor = panel.querySelector('textarea') as HTMLTextAreaElement;
    fireEvent.input(editor, { target: { value: '# Guide\n\nunsaved' } });

    fireEvent.click(screen.getByRole('button', { name: /close guide\.md/i }));
    // The tab is still open, and the panel is asking on its own account.
    expect(workspaceTabs.openFileTabs.value).toEqual(['docs/guide.md']);
    expect(within(panel).getByText(/discard your unsaved changes/i)).toBeTruthy();

    fireEvent.click(within(panel).getByRole('button', { name: /keep editing/i }));
    expect(workspaceTabs.openFileTabs.value).toEqual(['docs/guide.md']);
    expect(panel.querySelector('textarea')).not.toBeNull();

    // Only an informed "Discard" ends the tab.
    fireEvent.click(screen.getByRole('button', { name: /close guide\.md/i }));
    fireEvent.click(within(panel).getByRole('button', { name: /^discard$/i }));
    expect(workspaceTabs.openFileTabs.value).toEqual([]);
  });

  it('refuses to move a dirty file between the dialog and a tab', async () => {
    installFetch({ 'docs/guide.md': '# Guide\n\nbody' });
    renderWorkbench();

    act(() => {
      store.fileViewerPath.value = 'docs/guide.md';
    });
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(await within(dialog).findByRole('button', { name: /edit/i }));
    await waitFor(() => expect(dialog.querySelector('textarea')).not.toBeNull());
    const editor = dialog.querySelector('textarea') as HTMLTextAreaElement;
    fireEvent.input(editor, { target: { value: '# Edited' } });

    const open = within(dialog).getByRole('button', { name: /open in tab/i });
    expect(open.getAttribute('aria-disabled')).toBe('true');
    fireEvent.click(open);
    // Still in the dialog, with the draft intact.
    expect(store.fileViewerPath.value).toBe('docs/guide.md');
    expect((dialog.querySelector('textarea') as HTMLTextAreaElement).value).toBe('# Edited');
  });
});
