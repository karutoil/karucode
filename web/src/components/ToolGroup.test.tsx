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

  it('draws a grouped shell command as a row in the rows view, keeping its output reachable', () => {
    renderGroup([tool(), command()]);
    fireEvent.click(head());

    const row = document.querySelector('.command-row');
    expect(row).not.toBeNull();
    expect(document.querySelector('.command-card')).toBeNull();
    expect(row?.textContent).toContain('cargo test');
    expect(row?.textContent).toContain('exit 0');

    fireEvent.click(screen.getByRole('button', { name: /cargo test/ }));
    expect(document.querySelector('.cmd-output')?.textContent).toContain('ok');
  });

  it('draws the same grouped shell command as a card in the cards view', () => {
    renderGroup([tool(), command()], 'card');
    fireEvent.click(head());

    const card = document.querySelector('.command-card');
    expect(card).not.toBeNull();
    expect(document.querySelector('.command-row')).toBeNull();
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

  /* A group is a live view of a run: calls arrive, start and fail after it is
   * mounted. Opening only from the *initial* state meant a group that was
   * collapsed when it rendered stayed shut when one of its calls started running
   * or failed - the two states the group exists to surface. These re-render the
   * same boundary with a changed call, which is what the reducer does. */
  describe('opening on a state change after mount', () => {
    const first = tool({ id: 't1', name: 'read_file' });
    const second = tool({ id: 't2', callId: 'call_2', name: 'grep' });

    it('opens when a member starts running (rows)', () => {
      const view = render(<ToolGroup calls={[first, second]} variant="row" />);
      expect(head().getAttribute('aria-expanded')).toBe('false');
      expect(document.querySelector('.tool-row')).toBeNull();

      view.rerender(
        <ToolGroup calls={[first, { ...second, status: 'running', durationMs: null }]} variant="row" />,
      );

      expect(head().getAttribute('aria-expanded')).toBe('true');
      expect(group().getAttribute('data-tone')).toBe('is-running');
      // The live call's row is on screen, not one click away.
      expect(document.querySelectorAll('.tool-row')).toHaveLength(2);
      expect(screen.getByText('1 running')).not.toBeNull();
    });

    it('opens when a member starts running (cards)', () => {
      const view = render(<ToolGroup calls={[first, second]} variant="card" />);
      expect(head().getAttribute('aria-expanded')).toBe('false');

      view.rerender(
        <ToolGroup calls={[first, { ...second, status: 'running', durationMs: null }]} variant="card" />,
      );

      expect(head().getAttribute('aria-expanded')).toBe('true');
      // Cards are the transcript style here: the running call is a card, not a row.
      expect(document.querySelectorAll('.tool-card')).toHaveLength(2);
      expect(document.querySelector('.tool-row')).toBeNull();
    });

    it('opens when a member fails after mount, and names the failure', () => {
      const view = render(<ToolGroup calls={[first, second]} variant="row" />);
      expect(head().getAttribute('aria-expanded')).toBe('false');

      view.rerender(
        <ToolGroup calls={[first, { ...second, status: 'error', output: 'permission denied' }]} variant="row" />,
      );

      expect(head().getAttribute('aria-expanded')).toBe('true');
      expect(group().getAttribute('data-tone')).toBe('is-error');
      expect(screen.getByText('1 failed')).not.toBeNull();
      // The failure is on screen, and its output is one click away rather than
      // hidden behind the group disclosure as well.
      const failedRow = screen.getByRole('button', { name: /^grep/ });
      expect(failedRow.closest('.tool-row')?.getAttribute('data-status')).toBe('error');
      fireEvent.click(failedRow);
      expect(document.querySelector('.tool-row[data-status="error"] .tool-body')?.textContent).toContain(
        'permission denied',
      );
    });

    it('opens when a running call turns into a failure', () => {
      const view = render(
        <ToolGroup calls={[first, { ...second, status: 'running', durationMs: null }]} variant="row" />,
      );
      expect(head().getAttribute('aria-expanded')).toBe('true');
      // The reader closes it while the call is still in flight...
      fireEvent.click(head());
      expect(head().getAttribute('aria-expanded')).toBe('false');

      // ...and the call failing re-opens it: a failure is never silent.
      view.rerender(
        <ToolGroup calls={[first, { ...second, status: 'error', output: 'exit 101' }]} variant="row" />,
      );
      expect(head().getAttribute('aria-expanded')).toBe('true');
      expect(group().getAttribute('data-tone')).toBe('is-error');
    });

    it('does not close a group the reader opened when the run settles', () => {
      const view = render(
        <ToolGroup calls={[first, { ...second, status: 'running', durationMs: null }]} variant="row" />,
      );
      // Opened by the transition, so the output below is the evidence it opened for.
      view.rerender(
        <ToolGroup calls={[first, { ...second, status: 'ok', output: 'done' }]} variant="row" />,
      );
      expect(head().getAttribute('aria-expanded')).toBe('true');
      expect(document.querySelectorAll('.tool-row')).toHaveLength(2);
    });
  });

  for (const variant of ['card', 'row'] as const) {
    describe(`${variant} urgency transitions`, () => {
      it('keeps a manual collapse when two running calls become one', () => {
        const first = tool({ id: 'a', status: 'running', durationMs: null });
        const second = tool({ id: 'b', status: 'running', durationMs: null });
        const view = renderGroup([first, second], variant);
        fireEvent.click(head());
        expect(head().getAttribute('aria-expanded')).toBe('false');

        view.rerender(
          <ToolGroup
            calls={[{ ...first, status: 'ok', durationMs: 100 }, second]}
            variant={variant}
          />,
        );
        expect(head().getAttribute('aria-expanded')).toBe('false');
      });

      it('opens when running work moves to a different call at the same count', () => {
        const first = tool({ id: 'a', status: 'running', durationMs: null });
        const second = tool({ id: 'b', status: 'ok' });
        const view = renderGroup([first, second], variant);
        fireEvent.click(head());
        expect(head().getAttribute('aria-expanded')).toBe('false');

        view.rerender(
          <ToolGroup
            calls={[
              { ...first, status: 'ok', durationMs: 100 },
              { ...second, status: 'running', durationMs: null },
            ]}
            variant={variant}
          />,
        );
        expect(head().getAttribute('aria-expanded')).toBe('true');
      });

      it('opens when a running call becomes an error', () => {
        const call = tool({ id: 'a', status: 'running', durationMs: null });
        const view = renderGroup([call], variant);
        fireEvent.click(head());
        expect(head().getAttribute('aria-expanded')).toBe('false');
        view.rerender(
          <ToolGroup calls={[{ ...call, status: 'error', durationMs: 100 }]} variant={variant} />,
        );
        expect(head().getAttribute('aria-expanded')).toBe('true');
      });

      it('opens when a finished call newly reports an error', () => {
        const call = tool({ id: 'a', status: 'ok' });
        const view = renderGroup([call], variant);
        expect(head().getAttribute('aria-expanded')).toBe('false');
        view.rerender(<ToolGroup calls={[{ ...call, status: 'error' }]} variant={variant} />);
        expect(head().getAttribute('aria-expanded')).toBe('true');
      });
    });
  }
});
