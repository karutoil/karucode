import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { DiffView } from './DiffView';

const DIFF = [
  'diff --git a/src/loader.rs b/src/loader.rs',
  'index 111..222 100644',
  '--- a/src/loader.rs',
  '+++ b/src/loader.rs',
  '@@ -38,3 +38,4 @@',
  ' impl PluginRegistry {',
  '-    pub fn load(&mut self, path: PathBuf) {',
  '+    pub async fn load(&mut self, manifest: &PluginManifest)',
  '+        -> Result<PluginHandle, PluginError> {',
  '         self.validate(manifest)?;',
  '',
].join('\n');

describe('DiffView', () => {
  it('renders each diff line with the tone it deserves', () => {
    render(<DiffView diff={DIFF} />);
    expect(screen.getByRole('group', { name: 'Unified diff' })).not.toBeNull();

    expect(screen.getByText(/pub async fn load/).closest('.diff-line')?.className).toContain('add');
    expect(screen.getByText(/pub fn load/).closest('.diff-line')?.className).toContain('del');
    expect(screen.getByText(/impl PluginRegistry/).closest('.diff-line')?.className).toContain('ctx');
    expect(screen.getByText(/^@@/).closest('.diff-line')?.className).toContain('hunk');
  });

  it('prints the mock gutter number for each hunk line', () => {
    render(<DiffView diff={DIFF} />);
    const context = screen.getByText(/impl PluginRegistry/).closest('.diff-line');
    expect(context?.querySelector('i')?.textContent).toBe('38');

    const added = screen.getByText(/pub async fn load/).closest('.diff-line');
    expect(added?.querySelector('i')?.textContent).toBe('39');

    const removed = screen.getByText(/pub fn load/).closest('.diff-line');
    expect(removed?.querySelector('i')?.textContent).toBe('39');

    // Hunk headers carry no line number.
    expect(screen.getByText(/^@@/).closest('.diff-line')?.querySelector('i')?.textContent).toBe('');
  });

  it('spells out the diff tone for screen readers', () => {
    render(<DiffView diff={DIFF} />);
    const added = screen.getByText(/pub async fn load/).closest('.diff-line');
    expect(added?.querySelector('.sr-only')?.textContent).toContain('Added');
    const removed = screen.getByText(/pub fn load/).closest('.diff-line');
    expect(removed?.querySelector('.sr-only')?.textContent).toContain('Removed');
  });

  it('stays a bare diff unless a header is requested', () => {
    render(<DiffView diff={DIFF} />);
    expect(document.querySelector('.diff-head')).toBeNull();
    expect(document.querySelector('.diff-footer')).toBeNull();
  });

  it('shows the mock file header with added/removed counts when given a path', () => {
    render(<DiffView diff={DIFF} path="src/loader.rs" />);
    expect(screen.getByText('src/loader.rs')).not.toBeNull();
    expect(screen.getByText('+2')).not.toBeNull();
    expect(screen.getByText('−1')).not.toBeNull();
  });

  it('gives the card its footer: how many files changed', () => {
    render(<DiffView diff={DIFF} path="src/loader.rs" />);
    const footer = document.querySelector('.diff-footer');
    expect(footer?.textContent).toContain('1 file modified');
    expect(screen.queryByRole('button', { name: /Review changes/ })).toBeNull();
  });

  it('pluralises the file count and offers review when wired', () => {
    const onReview = vi.fn();
    render(<DiffView diff={DIFF} path="src/loader.rs" fileCount={3} onReview={onReview} />);
    expect(screen.getByText(/3 files modified/)).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: /Review changes/ }));
    expect(onReview).toHaveBeenCalledTimes(1);
  });

  it('derives the file name from the diff when only the header is requested', () => {
    render(<DiffView diff={DIFF} showHeader />);
    expect(screen.getByText('src/loader.rs')).not.toBeNull();
  });

  it('falls back to a plain unified-diff +++ header for the file name', () => {
    render(<DiffView diff={'--- a/notes.md\n+++ b/notes.md\n@@ -1 +1 @@\n-old\n+new\n'} showHeader />);
    expect(screen.getByText('notes.md')).not.toBeNull();
  });

  it('says so when there is nothing to show', () => {
    render(<DiffView diff={'  \n'} />);
    expect(screen.getByText('No diff for this file.')).not.toBeNull();
  });

  it('surfaces server-side truncation', () => {
    render(<DiffView diff={DIFF} truncated />);
    expect(screen.getByText('The server truncated this diff.')).not.toBeNull();
  });

  it('renders diff text as text, never as markup', () => {
    render(<DiffView diff={'@@ -1 +1 @@\n-<b>old</b>\n+<img src=x onerror=alert(1)>\n'} />);
    expect(screen.getByText('+<img src=x onerror=alert(1)>')).not.toBeNull();
    expect(document.querySelector('img')).toBeNull();
  });
});
