import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { useEffect, useRef } from 'preact/hooks';
import type { Terminal } from '../api/types';
import * as endpoints from '../api/endpoints';
import * as store from '../state/store';
import * as terminalState from '../state/terminals';
import { TerminalTab } from './TerminalTab';

// The surface is a lazily-imported chunk that mounts xterm, which draws to a
// canvas jsdom cannot provide. Its behaviour (mounting, fitting, reconnecting,
// writing bytes) is covered by the browser check and by
// `state/terminalSocket.test.ts`; this file is about the tab strip and the
// states, so the surface is stubbed.
//
// The stub keeps the panel's **meta row** — the state and the working directory
// — because `.terminal-cwd` is the only place the cwd is shown to the user:
// "two shells in different projects are distinguishable" is asserted here, and a
// stub that dropped the row made that assertion pass by accident, on the tab
// chip's combined `title — cwd` tooltip instead of the visible path.
vi.mock('./TerminalSurface', () => ({
  TerminalSurface: ({
    terminal,
    autoFocus = true,
  }: {
    terminal: Terminal;
    autoFocus?: boolean;
  }) => {
    const shellRef = useRef<HTMLTextAreaElement>(null);
    // A focus-capable stand-in matters here: a passive div cannot reproduce the
    // real surface's mount-time term.focus() stealing the next Arrow key.
    useEffect(() => {
      if (autoFocus) shellRef.current?.focus();
    }, [autoFocus]);
    return (
      <div
        id={`terminal-panel-${terminal.id}`}
        data-testid="terminal-surface"
        data-terminal={terminal.id}
      >
        <span class={`terminal-state status-${terminal.running ? 'open' : 'closed'}`}>
          {terminal.running ? 'open' : 'closed'}
        </span>
        <span class="terminal-cwd" title={terminal.cwd}>
          {terminal.cwd}
        </span>
        <textarea ref={shellRef} aria-label={`${terminal.id} shell`} />
      </div>
    );
  },
}));

vi.mock('../state/terminalSocket', () => ({
  TerminalSocket: class {
    connect(): void {}
    dispose(): void {}
    send(): void {}
    resize(): void {}
    reconnect(): void {}
    get isOpen(): boolean {
      return false;
    }
    get currentStatus(): string {
      return 'idle';
    }
  },
}));

function terminal(overrides: Partial<Terminal> = {}): Terminal {
  return {
    id: 't_one',
    title: 'karucode-1',
    cwd: '/work/karucode',
    project: 'p_1',
    position: 1,
    created_at: 1,
    updated_at: 1,
    last_used_at: 1,
    running: true,
    pid: 4242,
    attached: 1,
    ...overrides,
  };
}

/** jsdom has no layout, so each element's box is stubbed by hand. */
function stubBox(element: Element, box: Partial<DOMRect>): void {
  const rect = {
    x: 0,
    y: 0,
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
    width: 0,
    height: 0,
    toJSON: () => ({}),
    ...box,
  } as DOMRect;
  element.getBoundingClientRect = () => rect;
}

/** Pretend the server answers the list, create, delete, rename and restart. */
function stubServer(list: Terminal[]): void {
  vi.spyOn(endpoints, 'listTerminals').mockResolvedValue({
    enabled: true,
    terminals: list,
    max_terminals: 4,
    shell: '/bin/bash',
  });
  vi.spyOn(endpoints, 'createTerminal').mockImplementation(async () => ({
    terminal: terminal({ id: 't_new', title: 'new-2', position: 2 }),
  }));
  vi.spyOn(endpoints, 'deleteTerminal').mockImplementation(async (id: string) => ({
    deleted: id,
  }));
  vi.spyOn(endpoints, 'renameTerminal').mockImplementation(
    async (id: string, title: string | null) => ({
      terminal: terminal({ id, title: title ?? 'karucode-1' }),
    }),
  );
  vi.spyOn(endpoints, 'restartTerminal').mockImplementation(async (id: string) => ({
    terminal: terminal({ id, pid: 9999 }),
  }));
}

// The socket is not what these tests exercise; keep it inert so a surface can
// mount without a server.
vi.mock('../state/terminalSocket', () => ({
  TerminalSocket: class {
    connect(): void {}
    dispose(): void {}
    send(): void {}
    resize(): void {}
    reconnect(): void {}
    get isOpen(): boolean {
      return false;
    }
    get currentStatus(): string {
      return 'idle';
    }
  },
}));

describe('TerminalTab', () => {

  beforeEach(() => {
    terminalState.resetTerminalState();
    document.body.innerHTML = '';
  });

  afterEach(() => {
    vi.restoreAllMocks();
    terminalState.resetTerminalState();
    store.resetToastTimers();
  });

  it('explains that the terminal is disabled instead of offering a shell', async () => {
    vi.spyOn(endpoints, 'listTerminals').mockResolvedValue({
      enabled: false,
      terminals: [],
      max_terminals: 0,
    });
    render(<TerminalTab />);
    await waitFor(() => {
      expect(screen.getByText('The terminal is disabled')).toBeTruthy();
    });
    // No New button: there is nothing to create.
    expect(screen.queryByRole('button', { name: /New terminal/i })).toBeNull();
  });

  it('offers a first terminal when none exists', async () => {
    stubServer([]);
    render(<TerminalTab />);
    await waitFor(() => {
      expect(screen.getByText('No terminal yet')).toBeTruthy();
    });
    const create = screen.getByRole('button', { name: /New terminal/i });
    fireEvent.click(create);
    await waitFor(() => {
      expect(endpoints.createTerminal).toHaveBeenCalledTimes(1);
    });
  });

  it('lists one chip per terminal and marks the selected one', async () => {
    stubServer([terminal(), terminal({ id: 't_two', title: 'watcher', position: 2, running: false })]);
    render(<TerminalTab />);

    await waitFor(() => {
      expect(screen.getAllByRole('tab')).toHaveLength(2);
    });
    const tabs = screen.getAllByRole('tab');
    expect(tabs[0]!.getAttribute('aria-selected')).toBe('true');
    expect(tabs[0]!.textContent).toContain('karucode-1');
    // A terminal whose shell exited says so rather than pretending it is live.
    expect(tabs[1]!.textContent).toContain('exited');
    // The initial selected terminal is the first in the list.
    expect(terminalState.activeTerminal.value?.id).toBe('t_one');
  });

  it('selects another terminal on click', async () => {
    stubServer([terminal(), terminal({ id: 't_two', title: 'watcher', position: 2 })]);
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(2));

    fireEvent.click(screen.getAllByRole('tab')[1]!);
    expect(terminalState.activeTerminal.value?.id).toBe('t_two');
    // The selection is a per-browser preference, so it persists.
    expect(localStorage.getItem('karucode.activeTerminal')).toBe('t_two');
  });

  it('moves the selection with the arrow keys, wrapping at the ends', async () => {
    stubServer([
      terminal(),
      terminal({ id: 't_two', title: 'watcher', position: 2 }),
      terminal({ id: 't_three', title: 'third', position: 3 }),
    ]);
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(3));

    const tab = (id: string): HTMLElement => {
      const element = document.getElementById(`terminal-tab-${id}`);
      if (!element) throw new Error(`missing tab ${id}`);
      return element;
    };

    // ArrowRight from the first tab selects the second and takes focus with it,
    // so the next key is delivered to the newly selected tab.
    fireEvent.keyDown(tab('t_one'), { key: 'ArrowRight' });
    expect(terminalState.activeTerminal.value?.id).toBe('t_two');
    expect(document.activeElement).toBe(tab('t_two'));

    fireEvent.keyDown(tab('t_two'), { key: 'ArrowLeft' });
    expect(terminalState.activeTerminal.value?.id).toBe('t_one');

    // Wrapping: left from the first goes to the last, right from the last to the
    // first. The strip is a ring, not a dead end.
    fireEvent.keyDown(tab('t_one'), { key: 'ArrowLeft' });
    expect(terminalState.activeTerminal.value?.id).toBe('t_three');
    fireEvent.keyDown(tab('t_three'), { key: 'ArrowRight' });
    expect(terminalState.activeTerminal.value?.id).toBe('t_one');
  });

  it('keeps focus on the selected tab so a second Arrow does not enter the shell', async () => {
    stubServer([
      terminal(),
      terminal({ id: 't_two', title: 'watcher', position: 2 }),
      terminal({ id: 't_three', title: 'third', position: 3 }),
    ]);
    render(<TerminalTab />);
    await waitFor(() => {
      expect(screen.getByTestId('terminal-surface').getAttribute('data-terminal')).toBe('t_one');
    });

    const tab = (id: string): HTMLElement => {
      const element = document.getElementById(`terminal-tab-${id}`);
      if (!element) throw new Error(`missing tab ${id}`);
      return element;
    };
    tab('t_one').focus();
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowRight' });
    await waitFor(() => {
      expect(screen.getByTestId('terminal-surface').getAttribute('data-terminal')).toBe('t_two');
    });
    // The keyed surface mounted here. Its old unconditional term.focus() moved
    // activeElement to the shell, which made the next Arrow a PTY key.
    expect(document.activeElement).toBe(tab('t_two'));

    fireEvent.keyDown(document.activeElement!, { key: 'ArrowRight' });
    await waitFor(() => {
      expect(screen.getByTestId('terminal-surface').getAttribute('data-terminal')).toBe('t_three');
    });
    expect(terminalState.activeTerminal.value?.id).toBe('t_three');
    expect(document.activeElement).toBe(tab('t_three'));
  });

  it('still focuses the shell after pointer activation, including the already-active tab', async () => {
    stubServer([terminal(), terminal({ id: 't_two', title: 'watcher', position: 2 })]);
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(2));
    const first = document.getElementById('terminal-tab-t_one')!;
    const second = document.getElementById('terminal-tab-t_two')!;

    first.focus();
    fireEvent.keyDown(first, { key: 'ArrowRight' });
    await waitFor(() => {
      expect(screen.getByTestId('terminal-surface').getAttribute('data-terminal')).toBe('t_two');
    });
    expect(document.activeElement).toBe(second);

    // No surface remount happens on this click, so autoFocus must also react
    // when keyboard activation is followed by a pointer click on the same tab.
    fireEvent.click(second);
    await waitFor(() => {
      expect(document.activeElement).toBe(screen.getByLabelText('t_two shell'));
    });
  });

  it('jumps to the ends with Home and End', async () => {
    stubServer([
      terminal(),
      terminal({ id: 't_two', title: 'watcher', position: 2 }),
      terminal({ id: 't_three', title: 'third', position: 3 }),
    ]);
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(3));

    fireEvent.keyDown(document.getElementById('terminal-tab-t_one')!, { key: 'End' });
    expect(terminalState.activeTerminal.value?.id).toBe('t_three');
    fireEvent.keyDown(document.getElementById('terminal-tab-t_three')!, { key: 'Home' });
    expect(terminalState.activeTerminal.value?.id).toBe('t_one');
    expect(document.activeElement).toBe(document.getElementById('terminal-tab-t_one'));
  });

  it('leaves the selection alone for keys the tablist does not own', async () => {
    stubServer([terminal(), terminal({ id: 't_two', title: 'watcher', position: 2 })]);
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(2));

    fireEvent.keyDown(document.getElementById('terminal-tab-t_one')!, { key: 'ArrowDown' });
    fireEvent.keyDown(document.getElementById('terminal-tab-t_one')!, { key: 'a' });
    expect(terminalState.activeTerminal.value?.id).toBe('t_one');

    // Home/End inside the rename field move the caret, not the selection.
    fireEvent.click(screen.getByRole('button', { name: 'Rename karucode-1' }));
    const input = await screen.findByLabelText('Rename karucode-1');
    fireEvent.keyDown(input, { key: 'End' });
    fireEvent.keyDown(input, { key: 'ArrowLeft' });
    expect(terminalState.activeTerminal.value?.id).toBe('t_one');
  });

  it('points only the selected tab at a panel that exists', async () => {
    // Every tab used to carry `aria-controls`, but only the selected terminal's
    // surface is mounted - the others referenced ids that were never in the
    // document. The relationship now follows the mount.
    stubServer([terminal(), terminal({ id: 't_two', title: 'watcher', position: 2 })]);
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(2));

    const first = document.getElementById('terminal-tab-t_one')!;
    const second = document.getElementById('terminal-tab-t_two')!;
    expect(first.getAttribute('aria-controls')).toBe('terminal-panel-t_one');
    expect(second.hasAttribute('aria-controls')).toBe(false);
    expect(document.getElementById('terminal-panel-t_one')).not.toBeNull();
    expect(document.getElementById('terminal-panel-t_two')).toBeNull();

    fireEvent.click(second);
    await waitFor(() => {
      expect(document.getElementById('terminal-panel-t_two')).not.toBeNull();
    });
    expect(document.getElementById('terminal-tab-t_two')?.getAttribute('aria-controls')).toBe(
      'terminal-panel-t_two',
    );
    expect(document.getElementById('terminal-tab-t_one')?.hasAttribute('aria-controls')).toBe(false);
    expect(document.getElementById('terminal-panel-t_one')).toBeNull();
  });

  it('keeps the tab in the tablist while its name is being edited', async () => {
    // The panel is labelled by the tab; replacing the tab with an input while
    // renaming left `aria-labelledby` pointing at nothing.
    stubServer([terminal()]);
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(1));

    fireEvent.click(screen.getByRole('button', { name: 'Rename karucode-1' }));
    const input = await screen.findByLabelText('Rename karucode-1');
    expect(input).not.toBeNull();
    // Still one tab, still selected, still controlling the mounted panel.
    const tabs = screen.getAllByRole('tab');
    expect(tabs).toHaveLength(1);
    expect(tabs[0]!.getAttribute('aria-selected')).toBe('true');
    expect(tabs[0]!.getAttribute('aria-controls')).toBe('terminal-panel-t_one');
    expect(document.getElementById('terminal-panel-t_one')).not.toBeNull();
  });

  it('scrolls an off-screen selected tab into view', async () => {
    stubServer([
      terminal(),
      terminal({ id: 't_two', title: 'watcher', position: 2 }),
      terminal({ id: 't_three', title: 'third', position: 3 }),
    ]);
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(3));

    const strip = document.querySelector<HTMLElement>('.terminal-chips');
    const third = document.getElementById('terminal-tab-t_three');
    if (!strip || !third) throw new Error('missing strip');
    stubBox(strip, { left: 0, right: 200, width: 200 });
    stubBox(third, { left: 260, right: 330, width: 70 });

    fireEvent.click(third);
    // It hangs 130px past the right edge, so the strip scrolls by exactly that
    // and the newly selected tab is readable instead of hidden.
    await waitFor(() => expect(strip.scrollLeft).toBe(130));
  });

  it('scrolls a focused tab back into view without changing the selection', async () => {
    stubServer([
      terminal(),
      terminal({ id: 't_two', title: 'watcher', position: 2 }),
      terminal({ id: 't_three', title: 'third', position: 3 }),
    ]);
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(3));

    const strip = document.querySelector<HTMLElement>('.terminal-chips');
    const first = document.getElementById('terminal-tab-t_one');
    if (!strip || !first) throw new Error('missing strip');
    stubBox(strip, { left: 0, right: 200, width: 200 });
    stubBox(first, { left: -45, right: -12, width: 33 });
    strip.scrollLeft = 120;

    fireEvent.focusIn(first);
    expect(strip.scrollLeft).toBe(75);
    expect(terminalState.activeTerminal.value?.id).toBe('t_one');
  });

  it('closes a terminal and moves the selection off it', async () => {
    stubServer([terminal(), terminal({ id: 't_two', title: 'watcher', position: 2 })]);
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(2));

    fireEvent.click(screen.getByRole('button', { name: 'Close karucode-1' }));
    await waitFor(() => {
      expect(endpoints.deleteTerminal).toHaveBeenCalledWith('t_one');
    });
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(1));
    expect(terminalState.activeTerminal.value?.id).toBe('t_two');
  });

  it('releases the closed terminal\'s emulator so the map cannot grow forever', async () => {
    // The xterm instance is module state keyed by id and outlives the unmount,
    // so nothing frees it on its own: a session that opens and closes all 16
    // terminals used to hold 16 emulators, canvases and listener sets. The strip
    // calls the disposer the lazily-loaded surface registered.
    stubServer([terminal(), terminal({ id: 't_two', title: 'watcher', position: 2 })]);
    const disposed: string[] = [];
    terminalState.setTerminalEmulatorDisposer((id) => disposed.push(id));
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(2));

    fireEvent.click(screen.getByRole('button', { name: 'Close karucode-1' }));
    await waitFor(() => expect(disposed).toEqual(['t_one']));
    // The other terminal's emulator is untouched.
    expect(disposed).not.toContain('t_two');
  });

  it('renames a terminal from the tab strip, and a blank name restores the default', async () => {
    stubServer([terminal()]);
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(1));

    fireEvent.click(screen.getByRole('button', { name: 'Rename karucode-1' }));
    const input = await screen.findByLabelText('Rename karucode-1');
    fireEvent.input(input, { target: { value: 'build watcher' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => {
      expect(endpoints.renameTerminal).toHaveBeenCalledWith('t_one', 'build watcher');
    });

    // A blank value is the documented "back to the derived default".
    fireEvent.click(screen.getByRole('button', { name: 'Rename build watcher' }));
    const again = await screen.findByLabelText('Rename build watcher');
    fireEvent.input(again, { target: { value: '   ' } });
    fireEvent.keyDown(again, { key: 'Enter' });
    await waitFor(() => {
      expect(endpoints.renameTerminal).toHaveBeenLastCalledWith('t_one', null);
    });
  });

  it('cancels a rename on Escape without calling the API', async () => {
    stubServer([terminal()]);
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(1));
    fireEvent.click(screen.getByRole('button', { name: 'Rename karucode-1' }));
    const input = await screen.findByLabelText('Rename karucode-1');
    fireEvent.input(input, { target: { value: 'nope' } });
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(endpoints.renameTerminal).not.toHaveBeenCalled();
  });

  it('disables New at the terminal budget and says which setting', async () => {
    stubServer([
      terminal(),
      terminal({ id: 't_two', title: 'two', position: 2 }),
      terminal({ id: 't_three', title: 'three', position: 3 }),
      terminal({ id: 't_four', title: 'four', position: 4 }),
    ]);
    render(<TerminalTab />);
    await waitFor(() => expect(screen.getAllByRole('tab')).toHaveLength(4));
    const newButton = screen.getByRole('button', { name: /New/i });
    expect((newButton as HTMLButtonElement).disabled).toBe(true);
    expect(newButton.getAttribute('title')).toContain('terminals.max_terminals');
  });

  it('shows a readable error with a retry when the list cannot be fetched', async () => {
    vi.spyOn(endpoints, 'listTerminals').mockRejectedValue(new Error('backend down'));
    render(<TerminalTab />);
    await waitFor(() => {
      expect(screen.getByText('Could not load terminals')).toBeTruthy();
    });
    expect(screen.getByText('backend down')).toBeTruthy();
    expect(screen.getByRole('button', { name: /Try again/i })).toBeTruthy();
  });

  it('mounts the selected terminal surface, and only that one', async () => {
    stubServer([terminal(), terminal({ id: 't_two', title: 'watcher', position: 2 })]);
    render(<TerminalTab />);
    await waitFor(() => {
      expect(screen.getByTestId('terminal-surface').getAttribute('data-terminal')).toBe('t_one');
    });

    fireEvent.click(screen.getAllByRole('tab')[1]!);
    await waitFor(() => {
      expect(screen.getByTestId('terminal-surface').getAttribute('data-terminal')).toBe('t_two');
    });
  });

  it('shows the working directory so two shells in different projects are distinguishable', async () => {
    stubServer([terminal({ cwd: '/work/some-long-project-name' })]);
    render(<TerminalTab />);

    // The cwd is the visible text of the surface's meta row, not just a tooltip:
    // the point is telling two shells apart at a glance.
    await waitFor(() => {
      const cwd = document.querySelector('.terminal-cwd');
      expect(cwd?.textContent).toBe('/work/some-long-project-name');
      expect(cwd?.getAttribute('title')).toBe('/work/some-long-project-name');
    });

    // The tab chip's own tooltip still leads with the *title* — the label clips
    // at 150px, so dropping the name to make room for the path would make two
    // tabs with the same directory indistinguishable instead of distinguishable.
    const tab = screen.getByRole('tab');
    expect(tab.getAttribute('title')).toBe('karucode-1 — /work/some-long-project-name');
  });

  it('shows each terminal its own directory', async () => {
    stubServer([
      terminal({ cwd: '/work/app' }),
      terminal({ id: 't_two', title: 'watcher', position: 2, cwd: '/work/other' }),
    ]);
    render(<TerminalTab />);
    await waitFor(() => {
      expect(document.querySelector('.terminal-cwd')?.textContent).toBe('/work/app');
    });

    fireEvent.click(screen.getAllByRole('tab')[1]!);
    await waitFor(() => {
      expect(document.querySelector('.terminal-cwd')?.textContent).toBe('/work/other');
    });
  });
});
