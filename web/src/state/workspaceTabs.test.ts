import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { cleanup } from '@testing-library/preact';
import * as workspaceTabs from './workspaceTabs';

describe('workspaceTabs', () => {
  beforeEach(() => {
    workspaceTabs.resetWorkspaceTabs();
  });

  afterEach(() => {
    cleanup();
    workspaceTabs.resetWorkspaceTabs();
  });

  it('starts on the conversation with no file tabs', () => {
    expect(workspaceTabs.workspaceView.value).toBe('chat');
    expect(workspaceTabs.openFileTabs.value).toEqual([]);
  });

  it('opens a file as a selected tab and reuses the tab it already has', () => {
    workspaceTabs.openFileTab('docs/guide.md');
    expect(workspaceTabs.openFileTabs.value).toEqual(['docs/guide.md']);
    expect(workspaceTabs.fileViewPath(workspaceTabs.workspaceView.value)).toBe('docs/guide.md');

    workspaceTabs.selectWorkspaceView('chat');
    workspaceTabs.openFileTab('docs/guide.md');
    // One tab, not two, and the same file is selected again.
    expect(workspaceTabs.openFileTabs.value).toEqual(['docs/guide.md']);
    expect(workspaceTabs.fileViewPath(workspaceTabs.workspaceView.value)).toBe('docs/guide.md');
  });

  it('survives a path that cannot be used as a DOM id verbatim', () => {
    // A space, `#` and `%` all break an unencoded id; the view is encoded and the
    // path is what comes back out.
    const path = 'notes/two words #1 100%.md';
    const view = workspaceTabs.fileView(path);
    expect(view).not.toContain(' ');
    expect(view).not.toContain('#');
    expect(workspaceTabs.fileViewPath(view)).toBe(path);
    expect(workspaceTabs.isFileView(view)).toBe(true);
    expect(workspaceTabs.fileViewPath('chat')).toBeNull();
  });

  it('closes the selected tab onto its neighbour, then back to the conversation', () => {
    workspaceTabs.openFileTab('a.md');
    workspaceTabs.openFileTab('b.md');
    workspaceTabs.openFileTab('c.md');
    workspaceTabs.selectWorkspaceView(workspaceTabs.fileView('b.md'));

    // Closing a middle tab selects the one that took its place.
    workspaceTabs.closeFileTab('b.md');
    expect(workspaceTabs.openFileTabs.value).toEqual(['a.md', 'c.md']);
    expect(workspaceTabs.fileViewPath(workspaceTabs.workspaceView.value)).toBe('c.md');

    // Closing the last one falls back to the one before it...
    workspaceTabs.closeFileTab('c.md');
    expect(workspaceTabs.fileViewPath(workspaceTabs.workspaceView.value)).toBe('a.md');

    // ...and closing the only remaining tab returns to the conversation.
    workspaceTabs.closeFileTab('a.md');
    expect(workspaceTabs.openFileTabs.value).toEqual([]);
    expect(workspaceTabs.workspaceView.value).toBe('chat');
  });

  it('closes a tab that is not selected without moving the selected view', () => {
    workspaceTabs.openFileTab('a.md');
    workspaceTabs.openFileTab('b.md');
    workspaceTabs.closeFileTab('a.md');
    expect(workspaceTabs.openFileTabs.value).toEqual(['b.md']);
    expect(workspaceTabs.fileViewPath(workspaceTabs.workspaceView.value)).toBe('b.md');
  });

  it('lets a mounted panel refuse a close so a draft is never discarded silently', () => {
    workspaceTabs.openFileTab('a.md');
    const unregister = workspaceTabs.registerFileTabCloseGuard('a.md', () => true);

    workspaceTabs.requestCloseFileTab('a.md');
    // The guard handled it (it is asking); the tab is still there.
    expect(workspaceTabs.openFileTabs.value).toEqual(['a.md']);

    unregister();
    workspaceTabs.requestCloseFileTab('a.md');
    expect(workspaceTabs.openFileTabs.value).toEqual([]);
    expect(workspaceTabs.workspaceView.value).toBe('chat');
  });

  it('closeAllFileTabs clears the strip and returns to the conversation', () => {
    workspaceTabs.openFileTab('a.md');
    workspaceTabs.closeAllFileTabs();
    expect(workspaceTabs.openFileTabs.value).toEqual([]);
    expect(workspaceTabs.workspaceView.value).toBe('chat');
  });

  it('keeps the selected built-in view when no file tab is closed', () => {
    workspaceTabs.selectWorkspaceView('changes');
    workspaceTabs.closeAllFileTabs();
    expect(workspaceTabs.workspaceView.value).toBe('changes');
  });
});
