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
    fireEvent.scroll(scroller);

    rerender(
      <CommandCard item={command({ status: 'running', exitCode: null, stdout: 'a\nb\nc\n' })} defaultOpen />,
    );
    expect(scroller.scrollTop).toBe(500);
  });

  it('keeps a prior pinned reader following even when one burst is much taller than 96px', () => {
    const { container, rerender } = render(
      <CommandCard item={command({ status: 'running', exitCode: null, stdout: 'a\n' })} defaultOpen />,
    );
    const scroller = container.querySelector('.command-scroll');
    if (!scroller) throw new Error('command output scroller missing');
    const box = { scrollHeight: 500, clientHeight: 100 };
    fakeScroll(scroller, box, 400);
    fireEvent.scroll(scroller);

    box.scrollHeight = 10_000;
    rerender(
      <CommandCard
        item={command({ status: 'running', exitCode: null, stdout: 'a\n'.repeat(5_000) })}
        defaultOpen
      />,
    );
    expect(scroller.scrollTop).toBe(10_000);
  });

  it('never yanks the view away from a reader who scrolled up', () => {
    const { container, rerender } = render(
      <CommandCard item={command({ status: 'running', exitCode: null, stdout: 'a\n' })} defaultOpen />,
    );
    const scroller = container.querySelector('.command-scroll');
    if (!scroller) throw new Error('command output scroller missing');
    fakeScroll(scroller, { scrollHeight: 500, clientHeight: 100 }, 50);
    fireEvent.scroll(scroller);

    rerender(
      <CommandCard item={command({ status: 'running', exitCode: null, stdout: 'a\nb\nc\n' })} defaultOpen />,
    );
    expect(scroller.scrollTop).toBe(50);
  });

  it('stops following on an explicit upward scroll even inside the bottom tolerance', () => {
    const { container, rerender } = render(
      <CommandCard item={command({ status: 'running', exitCode: null, stdout: 'a\n' })} defaultOpen />,
    );
    const scroller = container.querySelector('.command-scroll');
    if (!scroller) throw new Error('command output scroller missing');
    const box = { scrollHeight: 500, clientHeight: 100 };
    fakeScroll(scroller, box, 400);
    fireEvent.scroll(scroller);
    scroller.scrollTop = 390;
    fireEvent.scroll(scroller);

    box.scrollHeight = 600;
    rerender(
      <CommandCard item={command({ status: 'running', exitCode: null, stdout: 'a\nb\nc\n' })} defaultOpen />,
    );
    expect(scroller.scrollTop).toBe(390);
  });

  it('renders stderr as text, never as markup', () => {
    render(<CommandCard item={command({ stderr: '<script>bad()</script>', status: 'error', exitCode: 1 })} defaultOpen />);
    expect(document.querySelector('.cmd-output script')).toBeNull();
    expect(screen.getByLabelText('stderr').textContent).toContain('<script>bad()</script>');
  });

  it('rates a running command on its own output and drops the badge when it settles', () => {
    const running = command({
      status: 'running',
      exitCode: null,
      durationMs: null,
      stdout: 'x'.repeat(8_000),
    });
    const { rerender } = render(<CommandCard item={running} />);
    // The command's stdio is the only text still arriving while it runs, so the
    // card measures that: 8 000 characters at the server's four-per-token ratio.
    const badge = document.querySelector('.command-summary .tps-badge');
    expect(badge?.querySelector('.tps-badge-count')?.textContent).toBe('2000');

    rerender(<CommandCard item={command({ stdout: 'x'.repeat(8_000) })} />);
    // Settled: the stream it described is over, so the badge goes with it.
    expect(document.querySelector('.tps-badge')).toBeNull();
  });
});

describe('CommandCard row variant', () => {
  function row(): HTMLElement {
    const element = document.querySelector('.command-row');
    if (!element) throw new Error('command row did not render');
    return element as HTMLElement;
  }

  function head(): HTMLButtonElement {
    return document.querySelector('.command-row-head') as HTMLButtonElement;
  }

  it('is one line: command, working directory, exit status and duration', () => {
    render(<CommandCard item={command()} variant="row" />);

    expect(document.querySelector('.command-card')).toBeNull();
    expect(row().querySelector('.command-text')?.textContent).toBe('cargo test');
    expect(row().querySelector('.command-cwd')?.textContent).toBe('crates/kc-core');
    expect(row().querySelector('.command-status')?.textContent).toBe('exit 0');
    expect(row().querySelector('.command-duration')?.textContent).toBe('1.3s');
    expect(head().getAttribute('aria-expanded')).toBe('false');
    expect(row().getAttribute('data-status')).toBe('ok');
  });

  it('collapses without rendering the output well at all', () => {
    render(<CommandCard item={command()} variant="row" />);
    expect(document.querySelector('.cmd-output')).toBeNull();
    expect(screen.queryByText('running 12 tests')).toBeNull();
  });

  it('keeps stdout and stderr one disclosure away, separated as in the card', () => {
    render(
      <CommandCard
        item={command({ status: 'error', exitCode: 101, stdout: 'running 1 test\n', stderr: 'panicked\n' })}
        variant="row"
      />,
    );

    fireEvent.click(head());

    expect(head().getAttribute('aria-expanded')).toBe('true');
    expect(row().getAttribute('data-open')).toBe('true');
    expect(screen.getByLabelText('stdout').textContent).toContain('running 1 test');
    expect(screen.getByLabelText('stderr').textContent).toContain('panicked');
    // The outcome is in the line and stays there; the output is the detail.
    expect(row().querySelector('.command-status')?.textContent).toBe('exit 101');

    fireEvent.click(head());
    expect(document.querySelector('.cmd-output')).toBeNull();
  });

  it('opens a running command itself, and says it is waiting', () => {
    render(
      <CommandCard
        item={command({ status: 'running', exitCode: null, stdout: '', durationMs: null })}
        variant="row"
        defaultOpen
      />,
    );
    expect(head().getAttribute('aria-expanded')).toBe('true');
    expect(row().querySelector('.command-status')?.textContent).toBe('running');
    expect(screen.getByText('Waiting for output…')).not.toBeNull();
  });

  it('opens a command that turns running after mount', () => {
    const { rerender } = render(<CommandCard item={command()} variant="row" />);
    expect(document.querySelector('.cmd-output')).toBeNull();

    rerender(
      <CommandCard
        item={command({ status: 'running', exitCode: null, durationMs: null })}
        variant="row"
        defaultOpen
      />,
    );
    expect(head().getAttribute('aria-expanded')).toBe('true');
  });

  it('labels a stopped command instead of inventing an exit code', () => {
    render(<CommandCard item={command({ status: 'stopped', exitCode: null, durationMs: null })} variant="row" />);
    expect(head().textContent).toContain('Stopped');
    expect(screen.queryByText(/exit/)).toBeNull();
  });

  it('carries the same live throughput as the card, in the row meta', () => {
    render(
      <CommandCard
        item={command({ status: 'running', exitCode: null, durationMs: null, stdout: 'x'.repeat(8_000) })}
        variant="row"
      />,
    );
    expect(row().querySelector('.command-row-meta .tps-badge-count')?.textContent).toBe('2000');
  });

  it('keeps the truncated badge and the full command in the row', () => {
    const long = 'cargo test --workspace --all-targets -- --nocapture';
    render(<CommandCard item={command({ command: long, truncated: true })} variant="row" />);
    expect(screen.getByText('truncated')).not.toBeNull();
    expect(head().getAttribute('title')).toBe(long);
    expect(row().querySelector('.command-text')?.getAttribute('title')).toBe(long);
  });

  it('wires the disclosure to the output region by id', () => {
    render(<CommandCard item={command()} variant="row" />);
    fireEvent.click(head());
    const panel = document.getElementById(head().getAttribute('aria-controls') ?? '');
    expect(panel?.className).toContain('cmd-output');
  });
});
