import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { ToolGroup } from './ToolGroup';
import type { CommandItem, ToolItem } from '../state/chat';
import type { CallItem } from '../state/transcriptLayout';
import { NOW } from '../test/fixtures';

let sequence = 0;

function tool(overrides: Partial<ToolItem> = {}): ToolItem {
  sequence += 1;
  return {
    kind: 'tool',
    id: `t${sequence}`,
    at: NOW,
    callId: `call_${sequence}`,
    name: 'read_file',
    args: { path: 'src/main.rs' },
    status: 'ok',
    output: 'fn main() {}',
    truncated: false,
    durationMs: 100,
    runId: 'r_x',
    ...overrides,
  };
}

function command(overrides: Partial<CommandItem> = {}): CommandItem {
  sequence += 1;
  return {
    kind: 'command',
    id: `c${sequence}`,
    at: NOW,
    callId: `cmd_${sequence}`,
    command: 'cargo test',
    cwd: '.',
    stdout: 'ok',
    stderr: '',
    output: '',
    exitCode: 0,
    status: 'ok',
    truncated: false,
    durationMs: 900,
    runId: 'r_x',
    ...overrides,
  };
}

function group(): HTMLElement {
  const element = document.querySelector('.tool-group');
  if (!element) throw new Error('tool group did not render');
  return element as HTMLElement;
}

function head(): HTMLButtonElement {
  return document.querySelector('.tool-group-head') as HTMLButtonElement;
}

function renderGroup(calls: CallItem[], variant: 'card' | 'row' = 'row') {
  return render(<ToolGroup calls={calls} variant={variant} />);
}

describe('ToolGroup', () => {
  it('summarises a run: names, count and total time', () => {
    renderGroup([tool({ name: 'read_file' }), tool({ name: 'grep' }), command()]);

    expect(document.querySelector('.tool-group-summary')?.textContent).toBe(
      'read_file, grep, run_command',
    );
    expect(document.querySelector('.tool-group-count')?.textContent).toBe('3 calls');
    expect(document.querySelector('.tool-group-duration')?.textContent).toBe('1.1s');
    expect(group().getAttribute('data-tone')).toBe('is-ok');
  });

  it('starts collapsed and opens to the individual calls', () => {
    renderGroup([tool({ name: 'read_file' }), tool({ name: 'grep' })]);

    expect(head().getAttribute('aria-expanded')).toBe('false');
    expect(document.querySelector('.tool-row')).toBeNull();

    fireEvent.click(head());

    expect(group().getAttribute('data-open')).toBe('true');
    expect(head().getAttribute('aria-expanded')).toBe('true');
    expect(document.querySelectorAll('.tool-row')).toHaveLength(2);
  });

  it('opens itself while a member is still running', () => {
    renderGroup([
      tool({ name: 'read_file' }),
      tool({ name: 'grep', status: 'running', durationMs: null }),
    ]);

    expect(head().getAttribute('aria-expanded')).toBe('true');
    expect(group().getAttribute('data-tone')).toBe('is-running');
    expect(screen.getByText('1 running')).not.toBeNull();
    // The running call is visible, not buried behind a click.
    expect(document.querySelectorAll('.tool-row')).toHaveLength(2);
  });

  it('opens itself after a failure and names it', () => {
    renderGroup([tool({ name: 'read_file' }), tool({ name: 'edit', status: 'error' })]);

    expect(group().getAttribute('data-tone')).toBe('is-error');
    expect(screen.getByText('1 failed')).not.toBeNull();
    expect(head().getAttribute('aria-expanded')).toBe('true');
  });

  it('renders a shell command with the command card, keeping its output reachable', () => {
    renderGroup([tool(), command()]);
    fireEvent.click(head());

    const card = document.querySelector('.command-card');
    expect(card).not.toBeNull();
    expect(card?.textContent).toContain('cargo test');
    fireEvent.click(screen.getByRole('button', { name: /cargo test/ }));
    expect(document.querySelector('.cmd-output')?.textContent).toContain('ok');
  });

  it('honours the transcript tool style inside the group', () => {
    renderGroup([tool(), tool({ name: 'grep' })], 'card');
    fireEvent.click(head());

    expect(document.querySelectorAll('.tool-card')).toHaveLength(2);
    expect(document.querySelector('.tool-row')).toBeNull();
  });

  it('drops the body from the DOM when collapsed again', () => {
    renderGroup([tool(), tool({ name: 'grep' })]);
    fireEvent.click(head());
    expect(document.querySelector('.tool-group-body')?.hasAttribute('hidden')).toBe(false);

    fireEvent.click(head());
    expect(document.querySelector('.tool-group-body')?.hasAttribute('hidden')).toBe(true);
    expect(document.querySelector('.tool-row')).toBeNull();
  });

  it('reports a stopped call without claiming success', () => {
    renderGroup([tool({ status: 'stopped', durationMs: null }), tool({ name: 'grep' })]);
    expect(group().getAttribute('data-tone')).toBe('is-stopped');
  });
});
