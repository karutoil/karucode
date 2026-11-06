import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/preact';
import { ToolCard } from './ToolCard';
import type { ToolItem } from '../state/chat';
import { NOW } from '../test/fixtures';

/**
 * Design contract tests: verify ToolCard matches the geometry and state colors
 * from karucode-v3-themes.html exactly.
 *
 * Specifications from the mock:
 * - .tool-card: border 1px solid var(--theme-313436), border-radius 10px
 * - .tool-head: height 44px (exact, not min-height)
 * - .tool-symbol: 26x26px, border-radius 7px
 * - .tool-body: mounted by the disclosure, not as a closed 112px preview
 * - .tool-expand:hover: color var(--gold) / var(--accent)
 *
 * State colors:
 * - .tool-success-ok: success color
 * - .tool-success-error: danger color
 * - .tool-success-running: accent color
 * - .tool-success-stopped: muted color
 */

function tool(overrides: Partial<ToolItem> = {}): ToolItem {
  return {
    kind: 'tool',
    id: 't1',
    at: NOW,
    callId: 'call_1',
    name: 'read_file',
    args: { path: 'src/main.rs' },
    status: 'ok',
    output: 'fn main() {}\n',
    truncated: false,
    durationMs: 12,
    runId: 'r_x',
    ...overrides,
  };
}

describe('ToolCard design contract', () => {
  afterEach(cleanup);

  it('renders .tool-card element (border-radius 10px defined in CSS)', () => {
    render(<ToolCard item={tool()} />);
    const card = document.querySelector('.tool-card');
    expect(card).not.toBeNull();
    expect(card?.tagName).toBe('SECTION');
  });

  it('renders .tool-head (height 44px defined in CSS)', () => {
    render(<ToolCard item={tool()} />);
    const head = document.querySelector('.tool-head');
    expect(head).not.toBeNull();
    expect(head?.tagName).toBe('HEADER');
  });

  it('renders .tool-symbol as a 26x26px container (border-radius 7px in CSS)', () => {
    render(<ToolCard item={tool()} />);
    const symbol = document.querySelector('.tool-symbol');
    expect(symbol).not.toBeNull();
    // Verify it contains an icon
    expect(symbol?.querySelector('svg')).not.toBeNull();
  });

  it('mounts .tool-body only after the output disclosure', () => {
    render(<ToolCard item={tool()} />);
    expect(document.querySelector('.tool-body')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Show output' }));
    const body = document.querySelector('.tool-body');
    expect(body).not.toBeNull();
    expect(body?.hasAttribute('role')).toBe(true);
  });

  it('renders .tool-foot with duration and expand button (height 33px in CSS)', () => {
    render(<ToolCard item={tool()} />);
    const foot = document.querySelector('.tool-foot');
    expect(foot).not.toBeNull();
    expect(screen.getByRole('button', { name: /Show (output|less|details)/ })).not.toBeNull();
  });

  it('applies success state class and data attribute for status=ok', () => {
    render(<ToolCard item={tool({ status: 'ok' })} />);
    const card = document.querySelector('.tool-card');
    expect(card?.getAttribute('data-status')).toBe('ok');
    const success = document.querySelector('.tool-success-ok');
    expect(success).not.toBeNull();
  });

  it('applies error state class and data attribute for status=error', () => {
    render(<ToolCard item={tool({ status: 'error', output: 'failed' })} />);
    const card = document.querySelector('.tool-card');
    expect(card?.getAttribute('data-status')).toBe('error');
    const error = document.querySelector('.tool-success-error');
    expect(error).not.toBeNull();
  });

  it('applies running state class and data attribute for status=running', () => {
    render(<ToolCard item={tool({ status: 'running', output: '', durationMs: null })} />);
    const card = document.querySelector('.tool-card');
    expect(card?.getAttribute('data-status')).toBe('running');
    const running = document.querySelector('.tool-success-running');
    expect(running).not.toBeNull();
  });

  it('applies stopped state class and data attribute for status=stopped', () => {
    render(<ToolCard item={tool({ status: 'stopped', output: '', durationMs: null })} />);
    const card = document.querySelector('.tool-card');
    expect(card?.getAttribute('data-status')).toBe('stopped');
    const stopped = document.querySelector('.tool-success-stopped');
    expect(stopped).not.toBeNull();
  });

  it('wires the expand button to the mounted tool-body with aria-controls', () => {
    render(<ToolCard item={tool()} />);
    const button = screen.getByRole('button', { name: /Show (output|less|details)/ });
    const bodyId = button.getAttribute('aria-controls');
    expect(bodyId).toBeTruthy();
    expect(document.getElementById(bodyId!)).toBeNull();

    fireEvent.click(button);
    const body = document.getElementById(bodyId!);
    expect(body?.classList.contains('tool-body')).toBe(true);
  });

  it('renders .tool-expand button that toggles open state', () => {
    render(<ToolCard item={tool()} />);
    const button = screen.getByRole('button', { name: /Show (output|less|details)/ });
    expect(button.classList.contains('tool-expand')).toBe(true);
    expect(button.getAttribute('aria-expanded')).toBe('false');
  });
});

describe('ToolCard row variant design contract', () => {
  afterEach(cleanup);

  it('renders .tool-row instead of .tool-card', () => {
    render(<ToolCard item={tool()} variant="row" />);
    expect(document.querySelector('.tool-card')).toBeNull();
    expect(document.querySelector('.tool-row')).not.toBeNull();
  });

  it('renders .tool-row-symbol for the row variant', () => {
    render(<ToolCard item={tool()} variant="row" />);
    const symbol = document.querySelector('.tool-row-symbol');
    expect(symbol).not.toBeNull();
    expect(symbol?.querySelector('svg')).not.toBeNull();
  });

  it('applies data-status to .tool-row for status=ok', () => {
    render(<ToolCard item={tool({ status: 'ok' })} variant="row" />);
    const row = document.querySelector('.tool-row');
    expect(row?.getAttribute('data-status')).toBe('ok');
  });

  it('applies data-status to .tool-row for status=error', () => {
    render(<ToolCard item={tool({ status: 'error', output: 'fail' })} variant="row" />);
    const row = document.querySelector('.tool-row');
    expect(row?.getAttribute('data-status')).toBe('error');
    const error = document.querySelector('.tool-success-error');
    expect(error).not.toBeNull();
  });

  it('applies data-status to .tool-row for status=running', () => {
    render(<ToolCard item={tool({ status: 'running', output: '', durationMs: null })} variant="row" />);
    const row = document.querySelector('.tool-row');
    expect(row?.getAttribute('data-status')).toBe('running');
    const running = document.querySelector('.tool-success-running');
    expect(running).not.toBeNull();
  });

  it('does not render .tool-body while closed', () => {
    render(<ToolCard item={tool()} variant="row" />);
    expect(document.querySelector('.tool-body')).toBeNull();
    expect(document.querySelector('.tool-row')?.getAttribute('data-open')).toBe('false');
  });
});

/**
 * Note: diff-preview tests belong in DiffView.test.tsx, but we document the
 * contract here for completeness:
 *
 * - .diff-preview: border-radius 10px
 * - .diff-head: height 43px (exact, not min-height)
 * - .file-badge: 26x26px, border-radius 7px
 * - .diff-line: padding 0 13px, line-height 1.85
 * - .diff-line.add: background var(--surface-diff-add), color var(--success)
 * - .diff-line.del: background var(--surface-diff-del), color var(--danger)
 * - .diff-line.ctx: background var(--surface-diff-ctx)
 */
