import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/preact';
import type { Terminal } from '../api/types';
import { selectTheme, updateTheme } from '../theme';
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
 * call left the element behind in the discarded host.
 *
 * It also exposes the mutable `options` object xterm really has (theme, fontSize,
 * fontFamily) and a `refresh()` record, because a live theme change is applied
 * to a *cached* instance through exactly those. Without them the retint test
 * could only observe a re-render, not that the emulator changed. */
const {
  openCalls,
  disposeCalls,
  focusCalls,
  optionSets,
  optionWrites,
  refreshCalls,
  fitCalls,
  resizeListeners,
  socketResizeCalls,
} = vi.hoisted(() => ({
  openCalls: [] as { element: HTMLElement | null; parent: HTMLElement }[],
  disposeCalls: [] as number[],
  focusCalls: [] as number[],
  optionSets: [] as Record<string, unknown>[],
  optionWrites: [] as { property: string; value: unknown }[],
  refreshCalls: [] as { start: number; end: number }[],
  fitCalls: [] as number[],
  resizeListeners: new Set<(size: { cols: number; rows: number }) => void>(),
  socketResizeCalls: [] as { cols: number; rows: number }[],
}));

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    element: HTMLElement | undefined;
    options: Record<string, unknown>;
    private written = '';

    constructor(options: Record<string, unknown> = {}) {
      this.options = new Proxy(
        { ...options },
        {
          set: (target, property, value: unknown) => {
            optionWrites.push({ property: String(property), value });
            return Reflect.set(target, property, value);
          },
        },
      );
      optionSets.push(this.options);
    }
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
      resizeListeners.clear();
    }
    write(data: string): void {
      this.written += data;
    }
    refresh(start: number, end: number): void {
      refreshCalls.push({ start, end });
    }
    reset(): void {}
    focus(): void {
      focusCalls.push(1);
    }
    onData(): { dispose: () => void } {
      return { dispose: () => {} };
    }
    onResize(listener: (size: { cols: number; rows: number }) => void): { dispose: () => void } {
      resizeListeners.add(listener);
      return { dispose: () => resizeListeners.delete(listener) };
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
    fit(): void {
      fitCalls.push(1);
    }
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
    resize(cols: number, rows: number): void {
      socketResizeCalls.push({ cols, rows });
    }
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
  selectTheme('ember');
  // The instance map is module state: release anything a previous test opened so
  // each test starts from "never opened".
  disposeTerminal('t_one');
  openCalls.length = 0;
  disposeCalls.length = 0;
  focusCalls.length = 0;
  optionSets.length = 0;
  optionWrites.length = 0;
  refreshCalls.length = 0;
  fitCalls.length = 0;
  resizeListeners.clear();
  socketResizeCalls.length = 0;
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
    // The appearance cache survives the keyed remount with the emulator: simply
    // moving hosts must not write the same options or repaint the screen.
    expect(optionWrites).toEqual([]);
    expect(refreshCalls).toEqual([]);
  });

  it('suppresses xterm autofocus for keyboard-origin activation', async () => {
    render(<TerminalSurface terminal={terminal()} autoFocus={false} />);
    await waitFor(() => expect(document.querySelector('.xterm')).not.toBeNull());
    expect(focusCalls).toEqual([]);
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

/* Live theme changes, through the emulator's own options.
 *
 * The app's palette is CSS custom properties; xterm draws to a canvas and reads
 * none of them. So the surface has to re-resolve the tokens and push them into
 * the *cached* instance's options - the instance is keyed by terminal id and
 * outlives the mount, so a value captured at first open would be stale forever.
 * The style probe is stubbed because jsdom does not resolve custom properties
 * from the token layer. */
const tokenValues: Record<string, string> = {};

function stubTokens(values: Record<string, string>): void {
  Object.assign(tokenValues, values);
  vi.spyOn(window, 'getComputedStyle').mockImplementation((() => ({
    getPropertyValue: (property: string) => tokenValues[property] ?? '',
  })) as unknown as typeof window.getComputedStyle);
}

/** Flush the surface's own next-frame fit before taking operation counts. */
async function flushTerminalMount(): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => window.requestAnimationFrame(() => resolve()));
  });
}

const DARK_TOKENS = {
  '--fs-body': '13px',
  '--font-mono': "'SFMono-Regular', Consolas, 'Liberation Mono', Menlo, ui-monospace, monospace",
  '--bg-sunken': '#16181d',
  '--bg': '#1a1c22',
  '--text': '#d7dbe3',
  '--muted': '#8b92a1',
  '--accent': '#7aa2f7',
  '--surface-hover': 'rgba(122,162,247,0.25)',
  '--danger': '#f7768e',
  '--success': '#9ece6a',
  '--warn': '#e0af68',
  '--info': '#7aa2f7',
};

const LIGHT_TOKENS = {
  ...DARK_TOKENS,
  '--bg-sunken': '#f6f4f1',
  '--bg': '#ffffff',
  '--text': '#1c1a18',
  '--muted': '#5c5852',
  '--accent': '#bb5f2c',
  '--surface-hover': 'rgba(187,95,44,0.18)',
  '--danger': '#a8323f',
  '--success': '#256b3f',
  '--warn': '#8a5a12',
  '--info': '#1f5f9e',
};

describe('TerminalSurface follows the live theme', () => {
  beforeEach(() => {
    stubTokens(DARK_TOKENS);
  });

  it('retints and re-measures the cached emulator instead of rebuilding it', async () => {
    render(<TerminalSurface terminal={terminal()} />);
    await waitFor(() => expect(document.querySelector('.xterm')).not.toBeNull());
    await flushTerminalMount();

    const instance = optionSets[0];
    if (!instance) throw new Error('the emulator was never constructed');
    const element = document.querySelector('.xterm');
    expect(instance.fontSize).toBe(13);
    expect((instance.theme as Record<string, string>).background).toBe('#16181d');
    expect(instance.fontFamily).toContain('SFMono-Regular');
    expect(openCalls).toHaveLength(1);
    const fitsBefore = fitCalls.length;
    const refreshesBefore = refreshCalls.length;

    // The Theme studio writes the tokens and then the signal; the stub stands in
    // for what the browser would resolve from `applyTheme()`.
    Object.assign(tokenValues, LIGHT_TOKENS, {
      '--fs-body': '12px',
      '--font-mono': "'Courier New', Courier, monospace",
    });
    act(() => {
      updateTheme({ mode: 'light', density: 'compact', mono: 'classic' });
    });

    // The same terminal object - same buffer, scrollback, cursor and socket.
    expect(optionSets[0]).toBe(instance);
    expect(terminalInstanceCount()).toBe(1);
    expect(openCalls).toHaveLength(1);
    expect(disposeCalls).toHaveLength(0);
    expect(document.querySelector('.xterm')).toBe(element);
    // ...with the new appearance, measured again by the fit addon.
    expect(instance.fontSize).toBe(12);
    expect(instance.fontFamily).toContain('Courier New');
    expect((instance.theme as Record<string, string>).background).toBe('#f6f4f1');
    // Both real metric and colour values changed, so each operation runs once.
    expect(fitCalls).toHaveLength(fitsBefore + 1);
    expect(refreshCalls).toHaveLength(refreshesBefore + 1);
  });

  it.each([
    ['compact', 12],
    ['comfortable', 13],
    ['spacious', 13.25],
  ] as const)('responds to %s density only when its font size changes', async (density, size) => {
    render(<TerminalSurface terminal={terminal()} />);
    await waitFor(() => expect(document.querySelector('.xterm')).not.toBeNull());
    await flushTerminalMount();
    const instance = optionSets[0];
    if (!instance) throw new Error('the emulator was never constructed');
    const fitsBefore = fitCalls.length;
    const refreshesBefore = refreshCalls.length;
    const writesBefore = optionWrites.length;

    tokenValues['--fs-body'] = `${size}px`;
    act(() => {
      updateTheme({ density });
    });

    expect(optionSets[0]).toBe(instance);
    expect(instance.fontSize).toBe(size);
    const fontChanged = size !== 13;
    // A real metric change fits once; a same-valued density signal is a no-op.
    expect(fitCalls).toHaveLength(fitsBefore + (fontChanged ? 1 : 0));
    expect(refreshCalls).toHaveLength(refreshesBefore);
    expect(optionWrites.slice(writesBefore).map((write) => write.property)).toEqual(
      fontChanged ? ['fontSize', 'fontFamily'] : [],
    );
    expect(openCalls).toHaveLength(1);
  });

  it('skips options, fit and repaint for unrelated activeTheme changes', async () => {
    render(<TerminalSurface terminal={terminal()} />);
    await waitFor(() => expect(document.querySelector('.xterm')).not.toBeNull());
    await flushTerminalMount();
    const instance = optionSets[0];
    if (!instance) throw new Error('the emulator was never constructed');
    const theme = instance.theme;
    // Initial construction already supplied these exact resolved values.
    expect(optionWrites).toEqual([]);
    expect(refreshCalls).toEqual([]);
    const fitsBefore = fitCalls.length;

    const edits = [
      () => updateTheme({ rounding: 22 }),
      () => updateTheme({ toolStyle: 'rows' }),
      () => updateTheme({ grouping: 'tools' }),
      () => updateTheme({ thinking: 'expanded' }),
      () => updateTheme({ customCSS: '.unrelated-panel { outline: 1px solid red; }' }),
      () => updateTheme({ name: 'Only the label changed' }),
    ];
    for (const edit of edits) {
      act(() => {
        edit();
      });
    }

    expect(optionWrites).toEqual([]);
    expect(fitCalls).toHaveLength(fitsBefore);
    expect(refreshCalls).toEqual([]);
    // In particular, no fresh theme object replaced the one xterm already had.
    expect(instance.theme).toBe(theme);
  });

  it('repaints a real custom-CSS colour change without re-fitting fonts', async () => {
    render(<TerminalSurface terminal={terminal()} />);
    await waitFor(() => expect(document.querySelector('.xterm')).not.toBeNull());
    await flushTerminalMount();
    const instance = optionSets[0];
    if (!instance) throw new Error('the emulator was never constructed');
    const fitsBefore = fitCalls.length;
    const refreshesBefore = refreshCalls.length;
    const writesBefore = optionWrites.length;

    // The comparison is against resolved CSS, not the customCSS string: custom
    // CSS which really overrides a terminal token must still reach the canvas.
    tokenValues['--accent'] = '#123456';
    act(() => {
      updateTheme({ customCSS: ':root { --accent: #123456; }' });
    });

    expect((instance.theme as Record<string, string>).cursor).toBe('#123456');
    expect(optionWrites.slice(writesBefore).map((write) => write.property)).toEqual(['theme']);
    expect(fitCalls).toHaveLength(fitsBefore);
    expect(refreshCalls).toHaveLength(refreshesBefore + 1);
  });

  it('keeps xterm resize events connected to the terminal socket', async () => {
    render(<TerminalSurface terminal={terminal()} />);
    await waitFor(() => expect(document.querySelector('.xterm')).not.toBeNull());
    await flushTerminalMount();
    expect(resizeListeners.size).toBe(1);

    act(() => {
      for (const listener of resizeListeners) listener({ cols: 132, rows: 43 });
    });
    expect(socketResizeCalls).toEqual([{ cols: 132, rows: 43 }]);
  });

  it.each([
    ['dark', 0],
    ['dark', 22],
    ['light', 0],
    ['light', 22],
  ] as const)('survives a %s theme at rounding %i', async (mode, rounding) => {
    render(<TerminalSurface terminal={terminal()} />);
    await waitFor(() => expect(document.querySelector('.xterm')).not.toBeNull());
    const instance = optionSets[0];
    if (!instance) throw new Error('the emulator was never constructed');
    const before = { ...instance };

    Object.assign(tokenValues, mode === 'light' ? LIGHT_TOKENS : DARK_TOKENS);
    act(() => {
      updateTheme({ mode, rounding });
    });

    // Rounding is a shape token and must not reach the emulator as a rebuild:
    // the terminal keeps its identity, its element and its socket.
    expect(optionSets[0]).toBe(instance);
    expect(openCalls).toHaveLength(1);
    expect(disposeCalls).toHaveLength(0);
    expect(instance.fontSize).toBe(before.fontSize);
    expect((instance.theme as Record<string, string>).background).toBe(
      mode === 'light' ? '#f6f4f1' : '#16181d',
    );
  });
});
