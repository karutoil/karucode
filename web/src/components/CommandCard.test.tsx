import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { CommandCard } from './CommandCard';
import type { CommandItem } from '../state/chat';
import { NOW } from '../test/fixtures';

function command(overrides: Partial<CommandItem> = {}): CommandItem {
  return {
    kind: 'command',
    id: 'c1',
    at: NOW,
    callId: 'call_1',
    command: 'cargo test',
    cwd: 'crates/kc-core',
    stdout: 'running 12 tests\n',
    stderr: '',
    output: '',
    exitCode: 0,
    status: 'ok',
    truncated: false,
    durationMs: 1310,
    runId: 'r_x',
    ...overrides,
  };
}

function card(): HTMLElement {
  const element = document.querySelector('.command-card');
  if (!element) throw new Error('command card did not render');
  return element as HTMLElement;
}

function summary(): HTMLButtonElement {
  return document.querySelector('.command-summary') as HTMLButtonElement;
}

function fakeScroll(element: Element, box: { scrollHeight: number; clientHeight: number }, top: number): void {
  let scrollTop = top;
  Object.defineProperty(element, 'scrollHeight', { configurable: true, get: () => box.scrollHeight });
  Object.defineProperty(element, 'clientHeight', { configurable: true, get: () => box.clientHeight });
  Object.defineProperty(element, 'scrollTop', {
    configurable: true,
    get: () => scrollTop,
    set: (value: number) => {
      scrollTop = value;
    },
  });
}

describe('CommandCard', () => {
  it('summarises the command, its directory and its exit status', () => {
    render(<CommandCard item={command()} />);

    expect(screen.getByText('cargo test')).not.toBeNull();
    expect(screen.getByText('crates/kc-core')).not.toBeNull();
    expect(screen.getByText('exit 0')).not.toBeNull();
    expect(screen.getByText('1.3s')).not.toBeNull();
    expect(summary().getAttribute('aria-expanded')).toBe('false');
    expect(card().getAttribute('data-open')).toBe('false');
  });

  it('hides a "." working directory, which carries no information', () => {
    render(<CommandCard item={command({ cwd: '.' })} />);
    expect(screen.queryByText('.')).toBeNull();
  });

  it('expands to show stdout, and collapses again', () => {
    render(<CommandCard item={command()} />);
    fireEvent.click(summary());
    expect(summary().getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByLabelText('stdout').textContent).toContain('running 12 tests');

    fireEvent.click(summary());
    expect(summary().getAttribute('aria-expanded')).toBe('false');
    expect(document.querySelector('.cmd-output')?.hasAttribute('hidden')).toBe(true);
  });

  it('wires the summary button to the output panel', () => {
    render(<CommandCard item={command()} />);
    const panel = document.getElementById(summary().getAttribute('aria-controls') ?? '');
    expect(panel).not.toBeNull();
    expect(panel?.className).toContain('cmd-output');
  });

  it('separates stdout and stderr so a failure is readable', () => {
    render(
      <CommandCard
        item={command({ status: 'error', exitCode: 101, stdout: 'running 1 test\n', stderr: 'panicked\n' })}
        defaultOpen
      />,
    );
    expect(screen.getByLabelText('stdout').textContent).toContain('running 1 test');
    expect(screen.getByLabelText('stderr').textContent).toContain('panicked');
    expect(screen.getByText('exit 101')).not.toBeNull();
    expect(card().getAttribute('data-status')).toBe('error');
  });

  it('falls back to restored history output when there are no streams', () => {
    render(<CommandCard item={command({ stdout: '', output: 'Finished in 1.31s' })} defaultOpen />);
    expect(screen.getByLabelText('output').textContent).toContain('Finished in 1.31s');
  });

  it('opens a running command and reports that it has no output yet', () => {
    render(<CommandCard item={command({ status: 'running', exitCode: null, stdout: '', durationMs: null })} defaultOpen />);
    expect(summary().getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByText('running')).not.toBeNull();
    expect(screen.getByText('Waiting for output…')).not.toBeNull();
  });

  it('says a finished command produced no output', () => {
    render(<CommandCard item={command({ stdout: '', output: '', durationMs: 40 })} defaultOpen />);
    expect(screen.getByText('No output.')).not.toBeNull();
  });

  it('labels a cancelled command instead of inventing an exit code', () => {
    render(<CommandCard item={command({ status: 'stopped', exitCode: null, durationMs: null })} />);
    expect(screen.getByText('Stopped')).not.toBeNull();
    expect(screen.queryByText(/exit/)).toBeNull();
  });

  it('marks truncated output', () => {
    render(<CommandCard item={command({ truncated: true })} />);
    expect(screen.getByText('truncated')).not.toBeNull();
  });

  it('follows a running command while the reader is at the bottom', () => {
    const { container, rerender } = render(
      <CommandCard item={command({ status: 'running', exitCode: null, stdout: 'a\n' })} defaultOpen />,
    );
    const scroller = container.querySelector('.command-scroll');
    if (!scroller) throw new Error('command output scroller missing');
    fakeScroll(scroller, { scrollHeight: 500, clientHeight: 100 }, 400);

    rerender(
      <CommandCard item={command({ status: 'running', exitCode: null, stdout: 'a\nb\nc\n' })} defaultOpen />,
    );
    expect(scroller.scrollTop).toBe(500);
  });

  it('never yanks the view away from a reader who scrolled up', () => {
    const { container, rerender } = render(
      <CommandCard item={command({ status: 'running', exitCode: null, stdout: 'a\n' })} defaultOpen />,
    );
    const scroller = container.querySelector('.command-scroll');
    if (!scroller) throw new Error('command output scroller missing');
    fakeScroll(scroller, { scrollHeight: 500, clientHeight: 100 }, 50);

    rerender(
      <CommandCard item={command({ status: 'running', exitCode: null, stdout: 'a\nb\nc\n' })} defaultOpen />,
    );
    expect(scroller.scrollTop).toBe(50);
  });

  it('renders stderr as text, never as markup', () => {
    render(<CommandCard item={command({ stderr: '<script>bad()</script>', status: 'error', exitCode: 1 })} defaultOpen />);
    expect(document.querySelector('.cmd-output script')).toBeNull();
    expect(screen.getByLabelText('stderr').textContent).toContain('<script>bad()</script>');
  });
});
