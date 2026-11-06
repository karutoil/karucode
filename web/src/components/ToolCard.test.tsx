import { describe, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/preact';
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

  it('mounts successful output only while expanded from the footer button', () => {
    render(<ToolCard item={tool()} />);
    expect(card().getAttribute('data-open')).toBe('false');
    // A closed successful card has no 112px raw-output preview to lay out.
    expect(document.querySelector('.tool-body')).toBeNull();
    expect(screen.queryByText('fn main() {}')).toBeNull();

    fireEvent.click(toggle());
    expect(card().getAttribute('data-open')).toBe('true');
    expect(toggle().getAttribute('aria-expanded')).toBe('true');
    expect(toggle().textContent).toContain('Show less');
    expect(document.querySelector('.tool-body')).not.toBeNull();
    expect(screen.getByText('fn main() {}')).not.toBeNull();

    fireEvent.click(toggle());
    expect(card().getAttribute('data-open')).toBe('false');
    expect(document.querySelector('.tool-body')).toBeNull();
  });

  it('numbers every output line like the mock', () => {
    render(<ToolCard item={tool({ output: 'first\nsecond\nthird' })} defaultOpen />);
    const numbers = [...document.querySelectorAll('.tool-line .num')].map((node) => node.textContent);
    expect(numbers).toEqual(['01', '02', '03']);
  });

  it('wires the disclosure button to the output region it mounts', () => {
    render(<ToolCard item={tool()} />);
    const bodyId = toggle().getAttribute('aria-controls');
    expect(bodyId).toBeTruthy();
    expect(document.getElementById(bodyId ?? '')).toBeNull();

    fireEvent.click(toggle());
    const body = document.getElementById(bodyId ?? '');
    expect(body).not.toBeNull();
    expect(body?.getAttribute('aria-label')).toBe('read_file output');
  });

  it('can start open when the caller asks for it', () => {
    render(<ToolCard item={tool()} defaultOpen />);
    expect(card().getAttribute('data-open')).toBe('true');
  });

  it('keeps the live waiting preview while the tool is still running', () => {
    render(<ToolCard item={tool({ status: 'running', output: '', durationMs: null })} />);
    expect(card().getAttribute('data-open')).toBe('false');
    expect(screen.getByText('Waiting for output…')).not.toBeNull();
    expect(screen.getByText('Running')).not.toBeNull();
    expect(document.querySelector('.tool-foot')?.textContent).toContain('Running…');
  });

  it('reports a finished tool with no output when its details are opened', () => {
    render(<ToolCard item={tool({ output: '' })} />);
    expect(document.querySelector('.tool-body')).toBeNull();
    expect(document.querySelector('.tool-expand')?.textContent).toContain('Show details');
    fireEvent.click(toggle());
    expect(document.querySelector('.tool-body')?.textContent).toContain('No output.');
  });

  it('marks a truncated result', () => {
    render(<ToolCard item={tool({ truncated: true })} />);
    expect(screen.getByText('truncated')).not.toBeNull();
    expect(card().getAttribute('data-status')).toBe('ok');
  });

  it('opens failures to the reason instead of showing only a status label', () => {
    render(<ToolCard item={tool({ status: 'error', output: 'permission denied' })} />);
    expect(card().getAttribute('data-status')).toBe('error');
    expect(card().getAttribute('data-open')).toBe('true');
    expect(screen.getByText('Failed')).not.toBeNull();
    expect(screen.getByText('permission denied')).not.toBeNull();
    expect(document.querySelector('.tool-foot')?.textContent).toContain('Failed in 12ms');
  });

  it('opens a card when an existing call fails, but respects a manual close', () => {
    const view = render(<ToolCard item={tool()} />);
    expect(document.querySelector('.tool-body')).toBeNull();

    view.rerender(<ToolCard item={tool({ status: 'error', output: 'permission denied' })} />);
    expect(card().getAttribute('data-open')).toBe('true');
    expect(screen.getByText('permission denied')).not.toBeNull();

    fireEvent.click(toggle());
    expect(document.querySelector('.tool-body')).toBeNull();
    view.rerender(<ToolCard item={tool({ status: 'error', output: 'still denied' })} />);
    expect(document.querySelector('.tool-body')).toBeNull();
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

  it('previews the image a tool returned for the model', () => {
    render(
      <ToolCard
        item={tool({
          output: 'shot.png — image/png (8 B)',
          images: [{ media_type: 'image/png', data: 'iVBORw0KGgo=', bytes: 8, name: 'shot.png' }],
        })}
      />,
    );
    const preview = screen.getByTestId('tool-image') as HTMLImageElement;
    expect(preview.getAttribute('src')).toBe('data:image/png;base64,iVBORw0KGgo=');
    expect(preview.getAttribute('alt')).toBe('shot.png · 8 B');
    // The thumbnail stays visible without expanding the text output.
    expect(card().getAttribute('data-open')).toBe('false');
    expect(screen.getByText('shot.png · 8 B')).not.toBeNull();
    expect(document.querySelector('.tool-body')).toBeNull();

    fireEvent.click(toggle());
    expect(screen.getByTestId('tool-image')).toBe(preview);
    expect(screen.getByText('shot.png — image/png (8 B)')).not.toBeNull();
  });

  it('opens a lightbox with the full image and closes it again', () => {
    render(
      <ToolCard
        item={tool({
          images: [{ media_type: 'image/jpeg', data: '/9j/4AAQ', bytes: 2048, name: 'ui/shot.jpg' }],
        })}
      />,
    );
    expect(screen.queryByRole('dialog')).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Enlarge ui/shot.jpg · 2.0 KB' }));
    const dialog = screen.getByRole('dialog');
    expect(dialog.textContent).toContain('ui/shot.jpg');
    const full = dialog.querySelector('.dialog-image') as HTMLImageElement;
    expect(full.getAttribute('src')).toBe('data:image/jpeg;base64,/9j/4AAQ');

    fireEvent.click(screen.getByRole('button', { name: 'Close dialog' }));
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('does not put a non-image media type into a data URL', () => {
    render(
      <ToolCard
        item={tool({ images: [{ media_type: 'image/svg+xml', data: 'PHN2Zz4=', bytes: 8, name: 'x.svg' }] })}
      />,
    );
    expect(screen.queryByTestId('tool-image')).toBeNull();
    expect(screen.getByText('image/svg+xml cannot be previewed here')).not.toBeNull();
  });
});

describe('ToolCard row variant', () => {
  function row(): HTMLElement {
    const element = document.querySelector('.tool-row');
    if (!element) throw new Error('tool row did not render');
    return element as HTMLElement;
  }

  it('is one line: name, argument, outcome and duration in the header', () => {
    render(<ToolCard item={tool()} variant="row" />);

    expect(document.querySelector('.tool-card')).toBeNull();
    expect(row().querySelector('.tool-row-name')?.textContent).toBe('read_file');
    expect(row().querySelector('.tool-row-args')?.textContent).toBe('src/main.rs');
    expect(row().querySelector('.tool-row-status')?.textContent).toBe('Done');
    expect(row().querySelector('.tool-row-duration')?.textContent).toBe('12ms');
    expect(row().getAttribute('data-status')).toBe('ok');
  });

  it('collapses without rendering the output body at all', () => {
    render(<ToolCard item={tool()} variant="row" />);
    // A collapsed row costs nothing: no numbered lines, no preview.
    expect(row().getAttribute('data-open')).toBe('false');
    expect(document.querySelector('.tool-body')).toBeNull();
    expect(screen.queryByText('fn main() {}')).toBeNull();
  });

  it('expands into the same numbered output the card shows', () => {
    render(<ToolCard item={tool({ output: 'first\nsecond\nthird' })} variant="row" />);
    const head = screen.getByRole('button', { name: /read_file/ });
    expect(head.getAttribute('aria-expanded')).toBe('false');

    fireEvent.click(head);
    expect(row().getAttribute('data-open')).toBe('true');
    expect(head.getAttribute('aria-expanded')).toBe('true');
    const numbers = [...document.querySelectorAll('.tool-line .num')].map((node) => node.textContent);
    expect(numbers).toEqual(['01', '02', '03']);
    expect(screen.getByText('Finished in 12ms')).not.toBeNull();

    // And it collapses again.
    fireEvent.click(head);
    expect(document.querySelector('.tool-body')).toBeNull();
  });

  it('keeps images visible without expanding the row', () => {
    render(
      <ToolCard
        item={tool({
          images: [{ media_type: 'image/png', data: 'iVBORw0KGgo=', bytes: 8, name: 'shot.png' }],
        })}
        variant="row"
      />,
    );
    expect(screen.getByTestId('tool-image')).not.toBeNull();
    expect(row().getAttribute('data-open')).toBe('false');
  });

  it('reports failure and a running call in the row itself', () => {
    render(<ToolCard item={tool({ status: 'error', output: 'nope' })} variant="row" />);
    expect(row().getAttribute('data-status')).toBe('error');
    expect(row().getAttribute('data-open')).toBe('true');
    expect(row().querySelector('.tool-row-status')?.className).toContain('tool-success-error');
    expect(row().querySelector('.tool-body')?.textContent).toContain('nope');

    cleanup();
    render(<ToolCard item={tool({ status: 'running', output: '', durationMs: null })} variant="row" />);
    expect(row().querySelector('.tool-row-status')?.textContent).toBe('Running');
    expect(row().querySelector('.tool-row-duration')).toBeNull();
  });

  it('shows "Waiting for output…" when a running row is opened', () => {
    render(<ToolCard item={tool({ status: 'running', output: '', durationMs: null })} variant="row" defaultOpen />);
    expect(document.querySelector('.tool-body')?.textContent).toContain('Waiting for output…');
  });
});
