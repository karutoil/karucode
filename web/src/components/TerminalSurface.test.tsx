import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/preact';
import type { Terminal } from '../api/types';
import * as terminalState from '../state/terminals';

/* The surface's **meta row**: the live state and the working directory.
 *
 * `TerminalTab.test.tsx` stubs the surface out (xterm draws to a canvas jsdom
 * cannot provide), so the only thing keeping it honest about the cwd was a stub.
 * The row is asserted here against the real component instead, with the socket
 * and the emulator mocked, so a change to `.terminal-cwd` fails in the file that
 * owns it rather than passing against a copy. */

/* The emulator stub reproduces the behaviour the shipped bug depended on:
 * `open(parent)` creates `.element` and appends it to `parent` *once*, and
 * early-returns on every later call - that is exactly what
 * `@xterm/xterm@5.5.0` `Terminal.open()` does (`src/browser/Terminal.ts:393`,
 * "If the terminal is already opened"). A no-op stub is why the re-mount bug
 * shipped: with `open()` doing nothing, nothing could observe that the second
 * call left the element behind in the discarded host. */
const { openCalls, disposeCalls } = vi.hoisted(() => ({
  openCalls: [] as { element: HTMLElement | null; parent: HTMLElement }[],
  disposeCalls: [] as number[],
}));

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    element: HTMLElement | undefined;
    private written = '';

    loadAddon(): void {}
    open(parent: HTMLElement): void {
      openCalls.push({ element: this.element ?? null, parent });
      // Faithful to xterm 5.5.0: already opened means do nothing.
      if (this.element) return;
      const element = document.createElement('div');
      element.className = 'xterm';
      this.element = element;
      parent.appendChild(element);
    }
    dispose(): void {
      disposeCalls.push(1);
      this.element?.remove();
      this.element = undefined;
    }
    write(data: string): void {
      this.written += data;
    }
    reset(): void {}
    focus(): void {}
    onData(): { dispose: () => void } {
      return { dispose: () => {} };
    }
    onResize(): { dispose: () => void } {
      return { dispose: () => {} };
    }
    get cols(): number {
      return 80;
    }
    get rows(): number {
      return 24;
    }
  },
}));

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit(): void {}
    dispose(): void {}
    proposeDimensions(): { cols: number; rows: number } {
      return { cols: 80, rows: 24 };
    }
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
      return true;
    }
    get currentStatus(): string {
      return 'open';
    }
    onData(): void {}
    onStatus(): void {}
  },
}));

const { TerminalSurface, disposeTerminal, terminalInstanceCount } = await import('./TerminalSurface');

function terminal(overrides: Partial<Terminal> = {}): Terminal {
  return {
    id: 't_one',
    title: 'karucode-1',
    cwd: '/work/some-long-project-name',
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

beforeEach(() => {
  terminalState.resetTerminalState();
  // The instance map is module state: release anything a previous test opened so
  // each test starts from "never opened".
  disposeTerminal('t_one');
  openCalls.length = 0;
  disposeCalls.length = 0;
});

afterEach(() => {
  vi.restoreAllMocks();
  terminalState.resetTerminalState();
});

describe('TerminalSurface meta row', () => {
  it('shows the working directory as visible text, with the full path as its tooltip', async () => {
    // The path is what tells two shells apart at a glance; a tooltip alone leaves
    // the reader hovering over tabs to find out which directory they are in.
    render(<TerminalSurface terminal={terminal()} />);
    await waitFor(() => {
      const cwd = document.querySelector('.terminal-cwd');
      expect(cwd).not.toBeNull();
      expect(cwd?.textContent).toBe('/work/some-long-project-name');
      expect(cwd?.getAttribute('title')).toBe('/work/some-long-project-name');
    });
  });

  it('renders the path unmangled even when the chip label would clip it', async () => {
    // `.terminal-chip-label` is capped at 150px with an ellipsis; the meta row is
    // not, so the full path survives whatever the strip does with the title.
    const long = '/work/a-really-quite-long-project-directory-name/with/a/subdirectory';
    render(<TerminalSurface terminal={terminal({ cwd: long })} />);
    await waitFor(() => {
      expect(document.querySelector('.terminal-cwd')?.textContent).toBe(long);
    });
  });

  it('labels the surface with the tab that controls it', async () => {
    // The panel is `aria-labelledby` the chip id; if the two sides ever disagree
    // the accessibility tree pairs the panel with the wrong tab.
    render(<TerminalSurface terminal={terminal()} />);
    await waitFor(() => {
      const panel = screen.getByRole('tabpanel');
      expect(panel.getAttribute('aria-labelledby')).toBe('terminal-tab-t_one');
      expect(panel.id).toBe('terminal-panel-t_one');
    });
  });
});

/* The blank-pane bug, reproduced.
 *
 * `TerminalTab` keys the surface by terminal id, so selecting another terminal
 * unmounts the surface and mounts a new one - a new component instance with a
 * new host `<div>`, but the *same* xterm object from the module-level
 * `instances` map. `Terminal.open()` early-returns once `.element` exists
 * (`@xterm/xterm@5.5.0`, src/browser/Terminal.ts:393), so calling `open(hostB)`
 * left the emulator inside the discarded host A: the pane was blank and could
 * not be typed into. Re-parenting is what moves it. */
describe('TerminalSurface keeps the emulator in the live host', () => {
  it('moves the existing element into the new host instead of re-opening', async () => {
    const first = render(<TerminalSurface terminal={terminal()} />);
    const hostA = first.container.querySelector<HTMLElement>('.terminal-host');
    expect(hostA).not.toBeNull();
    await waitFor(() => expect(hostA?.children.length).toBe(1));
    // The first mount is the only one that may call `open()`.
    expect(openCalls).toHaveLength(1);
    expect(openCalls[0]?.element).toBeNull();

    // Simulate the tab switch: the parent's `key` change tears this subtree down
    // and mounts a fresh surface with a fresh host for the same terminal id.
    // Re-rendering into the same container with a different key does exactly
    // that without reusing the old host node.
    first.rerender(<TerminalSurface key="t_one#2" terminal={terminal()} />);
    await waitFor(() => {
      const hostB = document.querySelector<HTMLElement>('.terminal-host');
      expect(hostB).not.toBeNull();
      expect(hostB).not.toBe(hostA);
      // The assertion the old stub could not make: the emulator moved. `element`
      // is the live emulator's root node, and it is now a child of host B.
      const moved = document.querySelector<HTMLElement>('.xterm');
      expect(moved).not.toBeNull();
      expect(moved?.parentElement).toBe(hostB);
      expect(hostB?.children.length).toBe(1);
      // ...and it is no longer in the host A that was thrown away.
      expect(hostA?.children.length).toBe(0);
    });
    // `open()` must not have been called a second time: the stub mirrors xterm,
    // which early-returns, so a second call would have been a silent no-op that
    // left the element behind.
    expect(openCalls).toHaveLength(1);
  });

  it('disposes the emulator when the terminal is closed', async () => {
    render(<TerminalSurface terminal={terminal()} />);
    await waitFor(() => expect(document.querySelector('.xterm')).not.toBeNull());
    expect(terminalInstanceCount()).toBe(1);
    // `state/terminals.closeTerminal` calls the disposer the surface registers,
    // so it reaches the same function through the strip's hook.
    terminalState.setTerminalEmulatorDisposer(disposeTerminal);
    disposeTerminal('t_one');
    expect(terminalInstanceCount()).toBe(0);
    expect(disposeCalls.length).toBeGreaterThan(0);
    // Idempotent: closing twice (or closing an id that never opened) is a no-op.
    disposeTerminal('t_one');
    disposeTerminal('t_never_opened');
    expect(terminalInstanceCount()).toBe(0);
  });
});
