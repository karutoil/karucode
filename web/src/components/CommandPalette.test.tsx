import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, createEvent, fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { configFixture, resetAppState } from '../test/fixtures';
import * as store from '../state/store';
import { CommandPalette } from './CommandPalette';

/**
 * A plugin's palette command is a real palette row, not a special case.
 *
 * It has to sort, filter, respond to arrow keys and run on Enter exactly like a
 * built-in entry — a second, plugin-only list would drift the moment somebody
 * touched the palette. It also names its plugin, because an action a person did
 * not expect should be attributable *before* it runs.
 */

const runPluginUiAction = vi.fn();

vi.mock('../api/endpoints', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/endpoints')>();
  return {
    ...actual,
    runPluginUiAction: (...args: unknown[]) => runPluginUiAction(...args),
  };
});

function openPaletteWithCommand(): void {
  store.config.value = {
    ...configFixture,
    plugin_ui: {
      contributions: [
        {
          plugin: 'kc-todo',
          version: '0.2.0',
          kind: 'command',
          id: 'plan',
          hosted_id: 'plugin__kc-todo__plan',
          label: 'Todo: summarise the plan',
          title: 'Ask the agent about the plan',
          message: 'Summarise the plan.',
        },
      ],
    } as never,
  };
  store.paletteOpen.value = true;
  render(<CommandPalette />);
}

describe('the palette chrome', () => {
  /* The palette is a list of controls, and its chrome is the design system's, not
   * its own: a shared group heading with its count, a marked match, one hint line.
   * These are the behaviours a reader notices when they are missing — "why is this
   * row here?", "how many are there?" — so they are tested here rather than left to
   * a screenshot. */

  beforeEach(() => {
    resetAppState();
  });

  afterEach(() => {
    cleanup();
    resetAppState();
  });

  function openPalette(): HTMLInputElement {
    store.paletteOpen.value = true;
    render(<CommandPalette />);
    return screen.getByRole('combobox', { name: 'Search commands' }) as HTMLInputElement;
  }

  it('marks the run the filter matched, so a surviving row explains itself', async () => {
    const input = openPalette();
    fireEvent.input(input, { target: { value: 'settings' } });

    await waitFor(() =>
      expect(screen.queryByRole('option', { name: /New session/ })).toBeNull(),
    );
    const row = screen.getByRole('option', { name: /Open settings/ });
    const mark = row.querySelector('.palette-mark');
    expect(mark?.textContent).toBe('settings');
    // Everything outside the match stays plain text: the mark is a reading aid,
    // not a restyled label.
    expect(row.textContent).toContain('Open ');
  });

  it('counts each group in its heading, and announces the whole list once', async () => {
    const input = openPalette();

    const heading = screen.getByText('Actions');
    expect(heading.querySelector('span')?.textContent).toBe('5');
    // The live region is the one place the total is announced. The footer used
    // to print it a third time, beside the group counts that already carry it.
    expect(document.querySelector('.palette-count')).toBeNull();
    expect(document.querySelector('[aria-live="polite"]')?.textContent).toBe('5 results');

    fireEvent.input(input, { target: { value: 'open' } });
    await waitFor(() =>
      expect(document.querySelector('[aria-live="polite"]')?.textContent).toBe('2 results'),
    );
    // The heading counts what is in front of the reader, not the catalog.
    expect(screen.getByText('Actions').querySelector('span')?.textContent).toBe('2');
  });

  it('hints the chord the rail actually answers to, not the one the browser owns', async () => {
    // `Mod+B` is the bookmarks bar in every browser (and Gecko's bookmarks
    // sidebar), so the rail moved to `Mod+\`. The palette hint is where a reader
    // learns that, and a hint for a chord that does nothing is worse than none.
    const input = openPalette();
    fireEvent.input(input, { target: { value: 'sessions rail' } });

    const row = await waitFor(() => screen.getByRole('option', { name: /sessions rail/ }));
    expect(row.querySelector('.palette-hint')?.textContent).toBe('Cmd/Ctrl+\\');
  });

  it('explains an empty result instead of showing a bare line', async () => {
    const input = openPalette();
    fireEvent.input(input, { target: { value: 'zzzz' } });

    await waitFor(() => expect(document.querySelector('.palette-empty')).toBeTruthy());
    expect(screen.getByText('No matching commands')).toBeTruthy();
    expect(screen.getByText(/Nothing in a name, a group or a shortcut matched/)).toBeTruthy();
    expect(document.querySelector('[aria-live="polite"]')?.textContent).toBe('0 results');
  });

  it('lists a session with its age under the Sessions heading', () => {
    store.sessions.value = [
      {
        id: 's_1',
        title: 'Search palette redesign',
        workspace: '/w',
        project_id: 'p_1',
        created_at: store.clock.value,
        updated_at: store.clock.value,
        message_count: 2,
        running: false,
      },
    ];
    openPalette();

    const row = screen.getByRole('option', { name: /Search palette redesign/ });
    // The age is the row's trailing meta; `just now` is what a fresh session is.
    expect(row.textContent).toContain('just now');
    expect(screen.getByText('Sessions').querySelector('span')?.textContent).toBe('1');
  });
});

describe('the palette and an IME', () => {
  /* Enter is two keys in one: the key that runs the highlighted row for a Latin
   * keyboard, and the key that commits a candidate for Japanese, Chinese and
   * Korean input. The palette used to take both, so a reader still composing a
   * word ran the first match - often "jump to another session" - and lost the
   * query with it. */

  beforeEach(() => {
    resetAppState();
  });

  afterEach(() => {
    cleanup();
    resetAppState();
  });

  it('leaves the Enter that commits a composition to the IME', async () => {
    const collapsed = store.railCollapsed.value;
    store.paletteOpen.value = true;
    render(<CommandPalette />);
    const input = screen.getByRole('combobox', { name: 'Search commands' });
    fireEvent.input(input, { target: { value: 'rail' } });
    await waitFor(() =>
      expect(screen.getByRole('option', { name: /sessions rail/ })).toBeTruthy(),
    );

    const composing = createEvent.keyDown(input, { key: 'Enter', isComposing: true });
    fireEvent(input, composing);
    // Not prevented: the key is the IME's, and it has to reach it.
    expect(composing.defaultPrevented).toBe(false);
    expect(store.railCollapsed.value).toBe(collapsed);
    expect(store.paletteOpen.value).toBe(true);

    // Firefox reports the same commit as keyCode 229 with `isComposing` false.
    expect(fireEvent.keyDown(input, { key: 'Enter', keyCode: 229 })).toBe(true);
    expect(store.railCollapsed.value).toBe(collapsed);
    expect(store.paletteOpen.value).toBe(true);

    // A commit that is over still runs the highlighted row.
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(store.railCollapsed.value).toBe(!collapsed);
    expect(store.paletteOpen.value).toBe(false);
  });
});

describe('the palette takes the caret when it opens', () => {
  /* SH-10/OV-11: the field inside a dialog was focused from a `setTimeout(…, 0)`.
   * iOS raises the software keyboard only for a `focus()` that happens inside the
   * gesture that opened the surface, and a timer is a new task — the palette
   * opened with a caret and no keyboard, and the reader had to tap the field
   * before they could type. `useFocusTrap` now focuses in the same frame the
   * portal is committed. */

  beforeEach(() => {
    resetAppState();
  });

  afterEach(() => {
    cleanup();
    resetAppState();
  });

  it('focuses the field in the render that creates it', () => {
    store.paletteOpen.value = true;
    render(<CommandPalette />);

    const input = screen.getByRole('combobox', { name: 'Search commands' });
    // No `waitFor` and no `await`: the caret is already in the field. With the
    // timer it was still on `<body>` at this point.
    expect(document.activeElement).toBe(input);
  });

  it('does it in a microtask when the palette opens over a running app', async () => {
    store.paletteOpen.value = false;
    render(<CommandPalette />);

    // The real flow: a keybinding flips the store and Preact re-renders in a
    // microtask. Microtasks only — a `setTimeout(0)` would not have run yet.
    store.paletteOpen.value = true;
    for (let tick = 0; tick < 5; tick += 1) await Promise.resolve();

    const input = screen.getByRole('combobox', { name: 'Search commands' });
    expect(document.activeElement).toBe(input);
  });
});

describe('plugin commands in the palette', () => {
  beforeEach(() => {
    resetAppState();
    runPluginUiAction.mockReset();
  });

  afterEach(() => {
    cleanup();
    resetAppState();
  });

  it('lists a contributed command under Plugins, naming its plugin', () => {
    openPaletteWithCommand();
    const option = screen.getByRole('option', { name: /Todo: summarise the plan/ });
    expect(option.textContent).toContain('kc-todo');
    expect(screen.getByText('Plugins')).toBeTruthy();
  });

  it('filters and runs it like any other row', async () => {
    openPaletteWithCommand();
    const input = screen.getByRole('combobox', { name: 'Search commands' });

    fireEvent.input(input, { target: { value: 'summarise' } });
    await waitFor(() =>
      expect(screen.queryByRole('option', { name: /New session/ })).toBeNull(),
    );

    // Enter runs the plugin's contribution: here a `message` one, so it posts a
    // user turn rather than an action.
    store.currentSessionId.value = 's_1';
    const fetchSpy = vi.fn(async () =>
      new Response(JSON.stringify({ run_id: 'r_1' }), {
        status: 202,
        headers: { 'Content-Type': 'application/json' },
      }),
    );
    vi.stubGlobal('fetch', fetchSpy);
    try {
      fireEvent.keyDown(input, { key: 'Enter' });
      await waitFor(() => expect(fetchSpy).toHaveBeenCalled());
      const body = (fetchSpy.mock.calls[0] as unknown as [string, RequestInit])[1]?.body;
      expect(String(body)).toContain('Summarise the plan.');
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('says so when a contributed command needs a session', async () => {
    openPaletteWithCommand();
    store.currentSessionId.value = null;
    fireEvent.click(screen.getByRole('option', { name: /Todo: summarise the plan/ }));
    await waitFor(() =>
      expect(store.toasts.value.map((toast) => toast.message).join(' ')).toContain(
        'Open a session',
      ),
    );
  });
});
