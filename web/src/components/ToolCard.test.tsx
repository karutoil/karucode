import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { ToolCard } from './ToolCard';
import type { ToolItem } from '../state/chat';
import { NOW } from '../test/fixtures';

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

function card(): HTMLElement {
  const element = document.querySelector('.tool-card');
  if (!element) throw new Error('tool card did not render');
  return element as HTMLElement;
}

function toggle(): HTMLButtonElement {
  return screen.getByRole('button', { name: /Show (output|less|details)/ }) as HTMLButtonElement;
}

describe('ToolCard', () => {
  it('summarises the call in the header: name, argument and outcome', () => {
    render(<ToolCard item={tool()} />);

    expect(screen.getByText('read_file')).not.toBeNull();
    expect(screen.getByText('src/main.rs')).not.toBeNull();
    expect(screen.getByText('Done')).not.toBeNull();
    expect(card().getAttribute('data-status')).toBe('ok');
  });

  it('keeps the full arguments available as a tooltip', () => {
    render(<ToolCard item={tool()} />);
    const command = document.querySelector('.tool-command');
    expect(command?.getAttribute('title')).toBe(JSON.stringify({ path: 'src/main.rs' }));
  });

  it('puts the duration in the footer, not the header', () => {
    render(<ToolCard item={tool()} />);
    expect(document.querySelector('.tool-foot')?.textContent).toContain('Finished in 12ms');
    expect(toggle().getAttribute('aria-expanded')).toBe('false');
  });

  it('clamps the output preview and expands from the footer button', () => {
    render(<ToolCard item={tool()} />);
    expect(card().getAttribute('data-open')).toBe('false');
    // The preview itself is visible; the footer button opens the full output.
    expect(screen.getByText('fn main() {}')).not.toBeNull();

    fireEvent.click(toggle());
    expect(card().getAttribute('data-open')).toBe('true');
    expect(toggle().getAttribute('aria-expanded')).toBe('true');
    expect(toggle().textContent).toContain('Show less');

    fireEvent.click(toggle());
    expect(card().getAttribute('data-open')).toBe('false');
  });

  it('numbers every output line like the mock', () => {
    render(<ToolCard item={tool({ output: 'first\nsecond\nthird' })} defaultOpen />);
    const numbers = [...document.querySelectorAll('.tool-line .num')].map((node) => node.textContent);
    expect(numbers).toEqual(['01', '02', '03']);
  });

  it('wires the disclosure button to the output region', () => {
    render(<ToolCard item={tool()} />);
    const body = document.getElementById(toggle().getAttribute('aria-controls') ?? '');
    expect(body).not.toBeNull();
    expect(body?.getAttribute('aria-label')).toBe('read_file output');
  });

  it('can start open when the caller asks for it', () => {
    render(<ToolCard item={tool()} defaultOpen />);
    expect(card().getAttribute('data-open')).toBe('true');
  });

  it('says it is waiting while the tool is still running', () => {
    render(<ToolCard item={tool({ status: 'running', output: '', durationMs: null })} defaultOpen />);
    expect(screen.getByText('Waiting for output…')).not.toBeNull();
    expect(screen.getByText('Running')).not.toBeNull();
    expect(document.querySelector('.tool-foot')?.textContent).toContain('Running…');
  });

  it('reports a finished tool with no output instead of an empty block', () => {
    render(<ToolCard item={tool({ output: '' })} />);
    expect(document.querySelector('.tool-body')?.textContent).toContain('No output.');
    expect(document.querySelector('.tool-expand')?.textContent).toContain('Show details');
  });

  it('marks a truncated result', () => {
    render(<ToolCard item={tool({ truncated: true })} />);
    expect(screen.getByText('truncated')).not.toBeNull();
    expect(card().getAttribute('data-status')).toBe('ok');
  });

  it('shows failures with the error tone and label', () => {
    render(<ToolCard item={tool({ status: 'error', output: 'permission denied' })} />);
    expect(card().getAttribute('data-status')).toBe('error');
    expect(screen.getByText('Failed')).not.toBeNull();
    expect(document.querySelector('.tool-foot')?.textContent).toContain('Failed in 12ms');
  });

  it('shows a stopped tool without pretending it finished', () => {
    render(<ToolCard item={tool({ status: 'stopped', output: '', durationMs: null })} />);
    expect(document.querySelector('.tool-success')?.textContent).toBe('Stopped');
    expect(document.querySelector('.tool-duration')?.textContent).toBe('Stopped');
  });

  it('renders output as text, never as markup', () => {
    render(<ToolCard item={tool({ output: '<img src=x onerror=alert(1)>' })} defaultOpen />);
    expect(document.querySelector('.tool-body img')).toBeNull();
    expect(screen.getByText('<img src=x onerror=alert(1)>')).not.toBeNull();
  });
});
