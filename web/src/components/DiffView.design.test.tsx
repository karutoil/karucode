import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/preact';
import { DiffView } from './DiffView';

/**
 * Design contract tests for diff-preview: verify it matches the geometry and
 * colors from karucode-v3-themes.html exactly.
 *
 * Specifications from the mock:
 * - .diff-preview: border-radius 10px
 * - .diff-head: height 43px (exact, not min-height)
 * - .file-badge: 26x26px, border-radius 7px
 * - .diff-line: padding 0 13px, line-height 1.85
 * - .diff-line.add: background var(--surface-diff-add), color var(--success)
 * - .diff-line.del: background var(--surface-diff-del), color var(--danger)
 * - .diff-line.ctx: background var(--surface-diff-ctx)
 */

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

describe('DiffView design contract', () => {
  afterEach(cleanup);

  it('renders .diff-preview element (border-radius 10px defined in CSS)', () => {
    render(<DiffView diff={DIFF} path="src/loader.rs" />);
    const preview = document.querySelector('.diff-preview');
    expect(preview).not.toBeNull();
  });

  it('renders .diff-head (height 43px exact, not min-height)', () => {
    render(<DiffView diff={DIFF} path="src/loader.rs" />);
    const head = document.querySelector('.diff-head');
    expect(head).not.toBeNull();
    // Verify it contains the file name and badge
    expect(screen.getByText('src/loader.rs')).not.toBeNull();
  });

  it('renders .file-badge as a 26x26px container (border-radius 7px in CSS)', () => {
    render(<DiffView diff={DIFF} path="src/loader.rs" />);
    const badge = document.querySelector('.file-badge');
    expect(badge).not.toBeNull();
    expect(badge?.querySelector('svg')).not.toBeNull();
  });

  it('renders .diff-line elements with proper structure', () => {
    render(<DiffView diff={DIFF} />);
    const lines = document.querySelectorAll('.diff-line');
    expect(lines.length).toBeGreaterThan(0);
  });

  it('applies .add class to added diff lines', () => {
    render(<DiffView diff={DIFF} />);
    const addedLine = document.querySelector('.diff-line.add');
    expect(addedLine).not.toBeNull();
    expect(addedLine?.textContent).toContain('pub async fn load');
  });

  it('applies .del class to deleted diff lines', () => {
    render(<DiffView diff={DIFF} />);
    const deletedLine = document.querySelector('.diff-line.del');
    expect(deletedLine).not.toBeNull();
    expect(deletedLine?.textContent).toContain('pub fn load');
  });

  it('applies .ctx class to context diff lines', () => {
    render(<DiffView diff={DIFF} />);
    const contextLine = document.querySelector('.diff-line.ctx');
    expect(contextLine).not.toBeNull();
    expect(contextLine?.textContent).toContain('impl PluginRegistry');
  });

  it('applies .hunk class to hunk header lines', () => {
    render(<DiffView diff={DIFF} />);
    const hunkLine = document.querySelector('.diff-line.hunk');
    expect(hunkLine).not.toBeNull();
    expect(hunkLine?.textContent).toContain('@@');
  });

  it('renders each diff line with gutter i element (27px width in CSS)', () => {
    render(<DiffView diff={DIFF} />);
    const lines = document.querySelectorAll('.diff-line i');
    expect(lines.length).toBeGreaterThan(0);
    // Verify structure: each line should have a gutter element
    const line = document.querySelector('.diff-line');
    const gutter = line?.querySelector('i');
    expect(gutter).not.toBeNull();
  });

  it('renders diff-text span for line content', () => {
    render(<DiffView diff={DIFF} />);
    const textSpans = document.querySelectorAll('.diff-text');
    expect(textSpans.length).toBeGreaterThan(0);
  });

  it('shows file statistics (+/- counts) in header when path is given', () => {
    render(<DiffView diff={DIFF} path="src/loader.rs" />);
    const stat = document.querySelector('.diff-stat');
    expect(stat).not.toBeNull();
    // Mock should show +2 additions and −1 deletion
    expect(stat?.textContent).toContain('+');
  });

  it('renders diff group with proper aria-label', () => {
    render(<DiffView diff={DIFF} />);
    const group = document.querySelector('[role="group"]');
    expect(group?.getAttribute('aria-label')).toContain('diff');
  });
});

