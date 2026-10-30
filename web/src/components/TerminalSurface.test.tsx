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

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    loadAddon(): void {}
    open(): void {}
    dispose(): void {}
    write(): void {}
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

const { TerminalSurface } = await import('./TerminalSurface');

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
