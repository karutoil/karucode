import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { CommandPalette } from './CommandPalette';
import { ThemeStudio } from './ThemeStudio';
import * as actions from '../state/actions';
import * as store from '../state/store';
import {
  DEFAULT_THEME,
  THEME_COLOR_GROUPS,
  THEME_COLOR_KEYS,
  THEME_STORAGE_KEY,
  THEME_TOKEN_ENTRIES,
  ROLE_PROPERTIES,
  activeTheme,
  buildTokenMap,
  contrastRatio,
  findBuiltin,
  resolvePalette,
  customThemes,
  deleteCustomTheme,
  draftEdited,
  exportThemeJSON,
  initTheme,
  parseThemeFile,
  resetTheme,
  saveThemeAsNew,
} from '../theme';

function persisted(): { selected?: string; draft?: { mode?: string } | null; custom?: unknown[] } {
  return JSON.parse(localStorage.getItem(THEME_STORAGE_KEY) ?? '{}') as {
    selected?: string;
    draft?: { mode?: string } | null;
    custom?: unknown[];
  };
}

function renderOpen(): HTMLElement {
  render(<ThemeStudio />);
  act(() => {
    actions.openThemeStudio();
  });
  return screen.getByTestId('theme-studio');
}

beforeEach(() => {
  localStorage.clear();
  resetTheme();
  store.themeStudioOpen.value = false;
  store.paletteOpen.value = false;
  store.resetToastTimers();
});

afterEach(() => {
  store.themeStudioOpen.value = false;
  store.paletteOpen.value = false;
  store.resetToastTimers();
});

describe('theme studio shell', () => {
  it('is not in the DOM until it is opened, then renders an accessible dialog', async () => {
    render(<ThemeStudio />);
    expect(screen.queryByTestId('theme-studio')).toBeNull();

    act(() => {
      actions.openThemeStudio();
    });

    const dialog = screen.getByRole('dialog', { name: 'Theme studio' });
    expect(dialog.getAttribute('aria-modal')).toBe('true');
    const describedBy = dialog.getAttribute('aria-describedby');
    expect(describedBy).toBeTruthy();
    expect(document.getElementById(describedBy as string)?.textContent).toMatch(/whole workspace/i);

    // All eight built-ins are offered and Ember is selected. The visible name
    // is also the accessible name (axe: label-content-name-mismatch).
    expect(dialog.querySelectorAll('.theme-preset')).toHaveLength(8);
    expect(within(dialog).getByRole('button', { name: 'Ember Dark' })).toBeTruthy();
    expect(screen.getByTestId('theme-preset-ember').getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByTestId('theme-preset-nocturne').getAttribute('aria-pressed')).toBe('false');
    expect((screen.getByTestId('theme-delete') as HTMLButtonElement).disabled).toBe(true);

    // Focus is moved inside the drawer.
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
  });

  it('opens from the store action used by the shell shortcut and closes on Escape', async () => {
    render(<ThemeStudio />);
    // `app.tsx` owns the Ctrl/Cmd+Shift+T shortcut and calls this action.
    act(() => {
      actions.toggleThemeStudio();
    });

    const dialog = screen.getByRole('dialog', { name: 'Theme studio' });
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));

    fireEvent.keyDown(document, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByTestId('theme-studio')).toBeNull());
    expect(store.themeStudioOpen.value).toBe(false);
  });

  it('closes on Escape even when the event targets the window', () => {
    renderOpen();
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(store.themeStudioOpen.value).toBe(false);
    expect(screen.queryByTestId('theme-studio')).toBeNull();
  });

  it('closes from the close button and the backdrop', () => {
    renderOpen();
    fireEvent.click(screen.getByRole('button', { name: 'Close theme studio' }));
    expect(screen.queryByTestId('theme-studio')).toBeNull();

    act(() => {
      actions.openThemeStudio();
    });
    const shade = document.querySelector('.theme-shade');
    expect(shade).not.toBeNull();
    fireEvent.click(shade as Element);
    expect(screen.queryByTestId('theme-studio')).toBeNull();
  });

  it('leaves only the command palette modal when the palette replaces the studio', () => {
    render(
      <>
        <ThemeStudio />
        <CommandPalette />
      </>,
    );
    act(() => {
      actions.openThemeStudio();
    });
    expect(screen.getByTestId('theme-studio').getAttribute('aria-modal')).toBe('true');

    act(() => {
      actions.togglePalette();
    });

    expect(store.themeStudioOpen.value).toBe(false);
    expect(store.paletteOpen.value).toBe(true);
    expect(screen.queryByTestId('theme-studio')).toBeNull();
    expect(screen.getByRole('dialog', { name: 'Command palette' })).toBeTruthy();
    expect(document.querySelectorAll('[aria-modal="true"]')).toHaveLength(1);
  });
});

describe('applying a built-in theme', () => {
  it('repaints the document, updates the shortcut target and persists', () => {
    renderOpen();

    fireEvent.click(screen.getByTestId('theme-preset-nocturne'));

    const root = document.documentElement;
    expect(root.dataset.theme).toBe('nocturne');
    expect(root.dataset.mode).toBe('dark');
    expect(root.style.getPropertyValue('--theme-bg')).toBe(
      resolvePalette(findBuiltin('nocturne')!).bg,
    );
    expect(persisted().selected).toBe('nocturne');
    expect(screen.getByTestId('theme-preset-nocturne').getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByTestId('theme-current-name').textContent).toBe('Nocturne');
  });

  it('applies a light theme to the whole UI', () => {
    renderOpen();
    fireEvent.click(screen.getByTestId('theme-preset-paper'));

    expect(document.documentElement.dataset.mode).toBe('light');
    expect(document.documentElement.style.colorScheme).toBe('light');
    expect(document.documentElement.style.getPropertyValue('--theme-text')).toBe(
      resolvePalette(findBuiltin('paper')!).text,
    );
  });
});

describe('colour editing', () => {
  it('exposes all eighteen editable colours, each with exactly one picker and one hex field', () => {
    renderOpen();

    // The palette input is `ThemeColors`; the studio groups are the only place a
    // user can reach those keys. They drifted once: `warn` was in the type, in
    // the resolver and in every built-in theme, and had no control at all, so 17
    // of the 18 colours were editable and Warning could not be changed from the
    // studio. This pins the count and the key set together.
    const grouped = THEME_COLOR_GROUPS.flatMap((group) => group.fields);
    expect(THEME_COLOR_KEYS).toHaveLength(18);
    expect(grouped).toHaveLength(18);
    expect(grouped.map((field) => field.key).sort()).toEqual([...THEME_COLOR_KEYS].sort());

    const applied = activeTheme.value.colors;
    for (const field of grouped) {
      const picker = screen.getByTestId(`theme-color-${field.key}`) as HTMLInputElement;
      expect(picker.type, field.key).toBe('color');
      expect(picker.value, field.key).toBe(applied[field.key]);
      expect(picker.getAttribute('aria-label')).toBe(`${field.label} colour picker`);
      // A colour control is described by the section copy it sits under.
      const describedBy = picker.getAttribute('aria-describedby') ?? '';
      expect(document.getElementById(describedBy)?.textContent, field.key).toBeTruthy();

      const hex = screen.getByTestId(`theme-hex-${field.key}`) as HTMLInputElement;
      expect(hex.value, field.key).toBe(applied[field.key].toUpperCase());
      expect(hex.getAttribute('aria-label')).toBe(`${field.label} hex value`);
    }
    expect(document.querySelectorAll('.theme-color-cell')).toHaveLength(18);
  });

  it('offers Warning as a first-class status colour next to Success, Error and Info', () => {
    renderOpen();
    const cell = screen.getByTestId('theme-hex-warn').closest('.theme-color-cell');
    if (!(cell instanceof HTMLElement)) throw new Error('no warning colour cell');
    expect(within(cell).getByText('Warning')).toBeTruthy();
    expect(within(cell).getByLabelText(/Warning hex value/)).toBeTruthy();
    expect(within(cell).getByLabelText(/Warning colour picker/)).toBeTruthy();
    // It lives with the other status inks, not in a section of its own.
    const section = cell.closest('.theme-section');
    expect(within(section as HTMLElement).getByText('Success')).toBeTruthy();
    expect(within(section as HTMLElement).getByText('Info')).toBeTruthy();

    fireEvent.input(screen.getByTestId('theme-hex-warn'), { target: { value: '#ffcc00' } });
    expect(activeTheme.value.colors.warn).toBe('#ffcc00');
    expect(document.documentElement.style.getPropertyValue('--theme-warn')).toBe(
      activeTheme.value.palette.warn,
    );
    expect(persisted().draft).toMatchObject({ colors: { warn: '#ffcc00' } });
  });

  it('applies a valid hex live, starts a draft and persists it', () => {
    renderOpen();
    const input = screen.getByTestId('theme-hex-bg');

    fireEvent.input(input, { target: { value: '#123456' } });

    expect(document.documentElement.style.getPropertyValue('--theme-bg')).toBe('#123456');
    expect(document.documentElement.dataset.theme).toBe('draft');
    expect(draftEdited.value).toBe(true);
    expect(persisted().draft).toMatchObject({ colors: { bg: '#123456' } });
    expect(screen.getByTestId('theme-autosave').textContent).toContain('Unsaved theme');
  });

  it('rejects an invalid hex without touching the applied theme', () => {
    renderOpen();
    const input = screen.getByTestId('theme-hex-bg');

    fireEvent.input(input, { target: { value: '#12' } });
    expect(input.getAttribute('aria-invalid')).toBe('true');
    expect(document.documentElement.style.getPropertyValue('--theme-bg')).toBe(
      resolvePalette(DEFAULT_THEME).bg,
    );
    expect(draftEdited.value).toBe(false);

    fireEvent.focusOut(input);
    expect(input.getAttribute('aria-invalid')).not.toBe('true');
  });

  it('applies the colour picker immediately', () => {
    renderOpen();
    fireEvent.input(screen.getByTestId('theme-color-accent'), { target: { value: '#ff5500' } });

    expect(document.documentElement.style.getPropertyValue('--theme-accent')).toBe('#ff5500');
    expect((screen.getByTestId('theme-hex-accent') as HTMLInputElement).value).toBe('#FF5500');
  });

  it('resets the palette back to the selected built-in', () => {
    renderOpen();
    fireEvent.click(screen.getByTestId('theme-preset-mint'));
    const original = document.documentElement.style.getPropertyValue('--theme-bg');

    fireEvent.input(screen.getByTestId('theme-hex-bg'), { target: { value: '#000000' } });
    expect(document.documentElement.style.getPropertyValue('--theme-bg')).toBe('#000000');

    fireEvent.click(screen.getByRole('button', { name: /reset colours/i }));
    expect(document.documentElement.style.getPropertyValue('--theme-bg')).toBe(original);
  });
});

describe('interface settings', () => {
  it('applies mode, density, fonts, radius and custom CSS, and reloads them', () => {
    renderOpen();

    fireEvent.click(screen.getByTestId('theme-mode-light'));
    expect(document.documentElement.dataset.mode).toBe('light');

    fireEvent.change(screen.getByTestId('theme-density'), { target: { value: 'compact' } });
    expect(document.documentElement.dataset.density).toBe('compact');
    expect(document.documentElement.style.getPropertyValue('--fs-body')).toBe('12px');

    fireEvent.change(screen.getByTestId('theme-font'), { target: { value: 'rounded' } });
    expect(document.documentElement.style.getPropertyValue('--font-ui')).toContain('Trebuchet');

    fireEvent.change(screen.getByTestId('theme-mono'), { target: { value: 'classic' } });
    expect(document.documentElement.style.getPropertyValue('--font-mono')).toContain('Courier New');

    fireEvent.input(screen.getByTestId('theme-radius'), { target: { value: '4' } });
    expect(document.documentElement.style.getPropertyValue('--radius')).toBe('4px');
    expect(screen.getByText('4px')).toBeTruthy();

    fireEvent.input(screen.getByTestId('theme-custom-css'), {
      target: { value: '.project-rail { backdrop-filter: blur(8px); }' },
    });
    expect(document.getElementById('kc-theme-custom-css')?.textContent).toContain('blur(8px)');

    expect(persisted().draft).toMatchObject({ density: 'compact', font: 'rounded' });

    // A reload re-applies what the studio persisted.
    initTheme();
    expect(document.documentElement.dataset.density).toBe('compact');
    expect(document.documentElement.style.getPropertyValue('--radius')).toBe('4px');
    expect(document.getElementById('kc-theme-custom-css')?.textContent).toContain('blur(8px)');
  });

  it('describes Custom CSS with both the override example and the trust note', () => {
    renderOpen();
    const textarea = screen.getByTestId('theme-custom-css');
    const describedBy = textarea.getAttribute('aria-describedby')?.split(/\s+/) ?? [];

    expect(describedBy).toHaveLength(2);
    expect(describedBy[0]).toMatch(/-css-help$/);
    expect(describedBy[1]).toMatch(/-css-trust-note$/);
    const help = document.getElementById(describedBy[0] as string);
    const trustNote = document.getElementById(describedBy[1] as string);
    expect(help?.textContent).toContain(':root { --theme-accent: #eaa06d !important; }');
    expect(trustNote?.textContent).toMatch(/Only paste CSS you trust/);
  });

  it('copies real theme variables that can override inline tokens in Custom CSS', async () => {
    const writeText = vi.spyOn(navigator.clipboard, 'writeText');
    renderOpen();
    fireEvent.input(screen.getByTestId('theme-custom-css'), {
      target: { value: '/* existing custom rule */' },
    });

    fireEvent.click(screen.getByRole('button', { name: 'Copy variables' }));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));

    const css = writeText.mock.calls[0]?.[0] as string;
    expect(css).toContain("/* !important lets Custom CSS replace the engine's inline token values. */");
    expect(css).toContain(`--theme-accent: ${DEFAULT_THEME.colors.accent} !important;`);
    expect(css).toContain(`--theme-syntax-keyword: ${DEFAULT_THEME.colors.syntaxKeyword} !important;`);
    expect(css).toContain(`--theme-line-strong: ${DEFAULT_THEME.colors.border} !important;`);
    const declarations = css.match(/^  --[a-z0-9-]+:/gim) ?? [];
    expect(declarations.length).toBeGreaterThanOrEqual(Object.keys(DEFAULT_THEME.colors).length);
    // Includes derived roles too; all of them must beat the engine's inline skin.
    expect(css.match(/ !important;$/gm)).toHaveLength(declarations.length);
    expect(css).not.toContain('--kc-');
    expect(css.endsWith('/* existing custom rule */')).toBe(true);
  });

  it('shows the live contrast status', () => {
    renderOpen();
    const contrast = screen.getByTestId('theme-contrast');
    expect(contrast.textContent).toMatch(/Contrast \d+\.\d:1/);
    expect(contrast.classList.contains('warning')).toBe(false);
  });

  it('prints the contrast of the palette it applied, not of the raw hex pair', () => {
    /* `resolvePalette` nudges primary text until it clears 11:1 on every surface,
     * so the two hexes in the form are inputs, not the painted result. The footer
     * has to report what is on screen or the number argues with the drawer. */
    renderOpen();
    fireEvent.input(screen.getByTestId('theme-hex-text'), { target: { value: '#3a3a3a' } });

    const raw = contrastRatio(activeTheme.value.colors.text, activeTheme.value.colors.bg);
    const applied = contrastRatio(activeTheme.value.palette.text, activeTheme.value.palette.bg);
    expect(raw).toBeLessThan(4.5);
    expect(screen.getByTestId('theme-contrast').textContent).toBe(
      `Contrast ${applied.toFixed(1)}:1 ✓`,
    );
    expect(screen.getByTestId('theme-contrast').classList.contains('warning')).toBe(false);
  });

  it('warns when the applied pair really is below AA', () => {
    renderOpen();
    // A light theme whose canvas is forced to black: the resolver can only walk
    // the ink toward black too, so this pair stays illegible and must be flagged.
    fireEvent.click(screen.getByTestId('theme-preset-paper'));
    fireEvent.input(screen.getByTestId('theme-hex-bg'), { target: { value: '#000000' } });
    fireEvent.input(screen.getByTestId('theme-hex-text'), { target: { value: '#000000' } });

    const contrast = screen.getByTestId('theme-contrast');
    expect(contrast.classList.contains('warning')).toBe(true);
    expect(contrast.textContent).toMatch(/· Low$/);
  });
});

describe('transcript settings', () => {
  it('offers the three view options with the defaults selected', () => {
    renderOpen();

    for (const [testId, value] of [
      ['theme-tool-style', 'cards'],
      ['theme-grouping', 'none'],
      ['theme-thinking', 'collapsed'],
    ] as const) {
      expect(screen.getByTestId(`${testId}-${value}`).getAttribute('aria-pressed')).toBe('true');
    }
  });

  it('applies each option to <html>, the preview and storage', () => {
    renderOpen();

    fireEvent.click(screen.getByTestId('theme-tool-style-rows'));
    expect(document.documentElement.dataset.toolStyle).toBe('rows');
    // The miniature follows: the one-line row is shown, the card is not.
    expect((document.querySelector('.theme-preview-row') as HTMLElement).hidden).toBe(false);
    expect((document.querySelector('.theme-preview-card') as HTMLElement).hidden).toBe(true);

    fireEvent.click(screen.getByTestId('theme-grouping-turns'));
    expect(document.documentElement.dataset.grouping).toBe('turns');
    expect(document.querySelector('.theme-preview-narration')?.getAttribute('data-on')).toBe('folded');

    fireEvent.click(screen.getByTestId('theme-thinking-preview'));
    expect(document.documentElement.dataset.thinking).toBe('preview');
    expect(document.querySelector('.theme-preview-thought')?.textContent).toContain('view options');

    expect(persisted().draft).toMatchObject({
      toolStyle: 'rows',
      grouping: 'turns',
      thinking: 'preview',
    });
  });

  it('explains the selected option rather than only naming it', () => {
    renderOpen();
    const groupingHint = () =>
      document
        .querySelector('[data-testid="theme-grouping"]')
        ?.parentElement?.querySelector('.theme-field-hint')?.textContent ?? '';

    // "None" is selected by default, and says what none means.
    expect(groupingHint()).toBe('Show every call where it happened.');

    fireEvent.click(screen.getByTestId('theme-grouping-tools'));
    expect(groupingHint()).toContain('summary line');

    fireEvent.click(screen.getByTestId('theme-grouping-turns'));
    expect(groupingHint()).toContain('finished turn');
  });

  it('restores the options after a reload', () => {
    renderOpen();
    fireEvent.click(screen.getByTestId('theme-tool-style-rows'));
    fireEvent.click(screen.getByTestId('theme-grouping-turns'));

    initTheme();
    expect(document.documentElement.dataset.toolStyle).toBe('rows');
    expect(document.documentElement.dataset.grouping).toBe('turns');
    expect(document.documentElement.dataset.thinking).toBe('collapsed');
  });

  it('carries the options through export and import so a shared theme arrives the same', () => {
    const json = exportThemeJSON({
      ...DEFAULT_THEME,
      toolStyle: 'rows',
      grouping: 'tools',
      thinking: 'expanded',
    });
    const imported = parseThemeFile(json);
    expect(imported.toolStyle).toBe('rows');
    expect(imported.grouping).toBe('tools');
    expect(imported.thinking).toBe('expanded');
  });
});

describe('save as new', () => {
  it('uses an inline name form instead of window.prompt', async () => {
    const promptSpy = vi.spyOn(window, 'prompt');
    renderOpen();

    fireEvent.click(screen.getByTestId('theme-save-as-new'));
    const input = screen.getByTestId('theme-name-input');
    await waitFor(() => expect(document.activeElement).toBe(input));

    fireEvent.input(input, { target: { value: 'Aurora' } });
    fireEvent.submit(input.closest('form') as HTMLFormElement);

    expect(promptSpy).not.toHaveBeenCalled();
    expect(customThemes.value).toHaveLength(1);
    expect(customThemes.value[0]?.name).toBe('Aurora');
    expect(screen.getByTestId('theme-current-name').textContent).toBe('Aurora');
    expect(
      screen.getByTestId(`theme-preset-${customThemes.value[0]?.id}`).getAttribute('aria-pressed'),
    ).toBe('true');
    expect(persisted().custom).toHaveLength(1);
    expect(store.toasts.value.at(-1)?.message).toContain('Aurora');
  });

  it('refuses an empty name and keeps the form open', () => {
    renderOpen();
    fireEvent.click(screen.getByTestId('theme-save-as-new'));
    const input = screen.getByTestId('theme-name-input');
    fireEvent.input(input, { target: { value: '   ' } });
    fireEvent.submit(input.closest('form') as HTMLFormElement);

    expect(customThemes.value).toHaveLength(0);
    expect(screen.getByTestId('theme-name-input')).toBeTruthy();
    expect(store.toasts.value.at(-1)?.tone).toBe('error');
  });
});

describe('delete', () => {
  it('confirms inline (no window.confirm) and falls back to Ember', () => {
    const confirmSpy = vi.spyOn(window, 'confirm');
    renderOpen();
    act(() => {
      saveThemeAsNew('Doomed');
    });

    expect((screen.getByTestId('theme-delete') as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(screen.getByTestId('theme-delete'));

    const alert = screen.getByRole('alertdialog');
    expect(within(alert).getByText(/Delete “Doomed”/)).toBeTruthy();
    fireEvent.click(screen.getByTestId('theme-delete-confirm'));

    expect(confirmSpy).not.toHaveBeenCalled();
    expect(customThemes.value).toHaveLength(0);
    expect(document.documentElement.dataset.theme).toBe('ember');
    expect(screen.queryByRole('alertdialog')).toBeNull();
  });

  it('cancels the confirmation without deleting', () => {
    renderOpen();
    act(() => {
      saveThemeAsNew('Keep me');
    });

    fireEvent.click(screen.getByTestId('theme-delete'));
    fireEvent.click(screen.getByRole('button', { name: 'Keep theme' }));

    expect(customThemes.value).toHaveLength(1);
    expect(screen.queryByRole('alertdialog')).toBeNull();
  });
});

describe('import and export', () => {
  it('imports a theme file from the hidden input', async () => {
    renderOpen();
    const file = new File([exportThemeJSON({ ...DEFAULT_THEME, name: 'From disk' })], 'theme.json', {
      type: 'application/json',
    });

    fireEvent.change(screen.getByTestId('theme-import-file'), { target: { files: [file] } });

    await waitFor(() => expect(customThemes.value).toHaveLength(1));
    expect(customThemes.value[0]?.name).toBe('From disk');
    expect(screen.getByTestId('theme-current-name').textContent).toBe('From disk');
    expect(store.toasts.value.at(-1)?.tone).toBe('success');
  });

  it('explains an invalid import and leaves the library untouched', async () => {
    renderOpen();
    const file = new File(['{}'], 'broken.json', { type: 'application/json' });

    fireEvent.change(screen.getByTestId('theme-import-file'), { target: { files: [file] } });

    await waitFor(() => expect(store.toasts.value.at(-1)?.tone).toBe('error'));
    expect(customThemes.value).toHaveLength(0);
    expect(store.toasts.value.at(-1)?.message).toContain('colours');
  });

  it('exports via a download without throwing', () => {
    const createObjectURL = vi.fn(() => 'blob:theme');
    const revokeObjectURL = vi.fn();
    Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: createObjectURL });
    Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: revokeObjectURL });
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => undefined);

    renderOpen();
    fireEvent.click(screen.getByTestId('theme-export'));

    expect(createObjectURL).toHaveBeenCalledTimes(1);
    expect(click).toHaveBeenCalledTimes(1);
    expect(store.toasts.value.at(-1)?.message).toContain('exported');
  });
});

describe('custom library', () => {
  it('keeps custom themes visible after a built-in is selected', () => {
    renderOpen();
    act(() => {
      saveThemeAsNew('Custom one');
    });
    expect(screen.getByText('Custom one', { selector: '.theme-preset-name' })).toBeTruthy();

    fireEvent.click(screen.getByTestId('theme-preset-ember'));
    expect(screen.getByText('Custom one', { selector: '.theme-preset-name' })).toBeTruthy();
    expect(screen.getByTestId('theme-preset-ember').getAttribute('aria-pressed')).toBe('true');
  });

  it('deletes via the engine API and rerenders the library', () => {
    renderOpen();
    act(() => {
      saveThemeAsNew('Temporary');
    });
    const id = customThemes.value[0]?.id as string;

    act(() => {
      deleteCustomTheme(id);
    });
    expect(screen.queryByText('Temporary', { selector: '.theme-preset-name' })).toBeNull();
  });
});

describe('mock fidelity', () => {
  it('uses the mock section copy and drops the extra hint block', () => {
    renderOpen();

    for (const heading of ['Backgrounds', 'Typography & accents', 'Syntax highlighting']) {
      expect(screen.getByRole('heading', { name: heading })).toBeTruthy();
    }
    expect(
      screen.getByText('Start from a look you love. Every part of the UI can be customized.'),
    ).toBeTruthy();
    expect(
      screen.getByText('Click a swatch or enter a hex code. Changes appear instantly.'),
    ).toBeTruthy();
    expect(
      screen.getByText('Fine-tune code colours independently of the application palette.'),
    ).toBeTruthy();
    expect(screen.getByText(/Full control: add your own selectors/)).toBeTruthy();

    // The mock describes the hex inputs with the section copy instead of a
    // separate hint paragraph.
    expect(document.querySelector('.theme-colors-hint')).toBeNull();
    const describedBy = screen.getByTestId('theme-hex-bg').getAttribute('aria-describedby');
    expect(document.getElementById(describedBy as string)?.textContent).toBe(
      "Make the workspace yours, from the project rail to the editor.",
    );
  });

  it('renders the mock footer: autosave, contrast and the four actions', () => {
    renderOpen();
    const footer = document.querySelector('.theme-studio-foot') as HTMLElement;

    expect(footer.querySelector('.theme-autosave')?.textContent).toContain(
      'Autosaved in this browser',
    );
    expect(footer.querySelector('.theme-autosave .theme-dot')).not.toBeNull();
    expect(footer.querySelector('.theme-contrast')?.textContent).toMatch(
      /Contrast \d+\.\d:1 (✓|· Low)/,
    );

    const buttons = Array.from(footer.querySelectorAll('.theme-foot-buttons .theme-btn'));
    expect(buttons.map((button) => button.textContent?.trim())).toEqual([
      'Import',
      'Export',
      '',
      'Save as new',
    ]);
    // The delete button is a plain .theme-btn in the mock, not a square icon button.
    expect(buttons[2]?.classList.contains('icon-only')).toBe(false);
    expect(buttons[2]?.getAttribute('aria-label')).toBe('Delete the current custom theme');
    expect(buttons[3]?.classList.contains('spacer')).toBe(true);
    expect(buttons[3]?.classList.contains('primary')).toBe(true);
  });

  it('renders the custom CSS details with a chevron instead of an "Optional" label', () => {
    renderOpen();
    const summary = document.querySelector('.theme-advanced summary') as HTMLElement;

    expect(summary.textContent?.trim()).toBe('Custom CSS');
    expect(summary.querySelector('svg')).not.toBeNull();
    expect(summary.textContent).not.toContain('Optional');
  });
});

describe('custom CSS is a trust boundary', () => {
  it('says that pasted or imported CSS can load remote resources and hide the UI', () => {
    renderOpen();
    fireEvent.click(document.querySelector('.theme-advanced summary') as HTMLElement);

    const note = screen.getByTestId('theme-css-trust');
    expect(note.getAttribute('role')).toBe('note');
    const text = note.textContent ?? '';
    expect(text).toMatch(/only paste CSS you trust/i);
    // The two consequences a user cannot otherwise predict.
    expect(text).toMatch(/remote fonts, images and beacons/i);
    expect(text).toMatch(/hide parts of the interface/i);
    // And the vector: an imported theme file carries its CSS with it.
    expect(text).toMatch(/imported theme carries its CSS/i);
  });

  it('copies the real --theme-* tokens and roles, not invented --kc-* names', async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>(async () => undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    renderOpen();
    fireEvent.click(document.querySelector('.theme-advanced summary') as HTMLElement);
    fireEvent.click(screen.getByRole('button', { name: /copy variables/i }));

    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    const code = writeText.mock.calls[0]?.[0] as string;

    // Every declaration is a property the app actually reads.
    const applied = buildTokenMap(activeTheme.value);
    const declared = [...code.matchAll(/^\s{2}(--[a-z-]+):\s(.+);$/gm)];
    expect(declared.length).toBe(THEME_TOKEN_ENTRIES.length + ROLE_PROPERTIES.length);
    for (const [, property, value] of declared) {
      expect(value).toMatch(/ !important$/);
      expect(applied[property!], property).toBe(value?.replace(/ !important$/, ''));
    }
    // The palette half is `--theme-*`; the fidelity roles are the derived half.
    const palette = declared.filter(([, property]) => property!.startsWith('--theme-'));
    expect(palette).toHaveLength(THEME_TOKEN_ENTRIES.length);
    expect(declared.some(([, property]) => property!.startsWith('--kc-'))).toBe(false);
    // The custom CSS travels with the block, so the copy is the whole theme.
    fireEvent.input(screen.getByTestId('theme-custom-css'), {
      target: { value: '.project-rail { filter: blur(2px); }' },
    });
    fireEvent.click(screen.getByRole('button', { name: /copy variables/i }));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(2));
    expect(writeText.mock.calls[1]?.[0]).toContain('.project-rail { filter: blur(2px); }');
    expect(store.toasts.value.at(-1)?.tone).toBe('success');
  });
});

describe('control descriptions are wired to their controls', () => {
  it('describes every transcript and appearance control with text that exists', () => {
    renderOpen();

    const groups = [
      'theme-tool-style',
      'theme-grouping',
      'theme-thinking',
    ].map((testId) => screen.getByTestId(testId));
    for (const group of groups) {
      const describedBy = group.getAttribute('aria-describedby') ?? '';
      expect(describedBy, group.dataset.testid).toBeTruthy();
      const hint = document.getElementById(describedBy);
      expect(hint?.classList.contains('theme-field-hint'), group.dataset.testid).toBe(true);
      const labelledBy = group.getAttribute('aria-labelledby') ?? '';
      expect(document.getElementById(labelledBy)?.textContent).toBeTruthy();
    }

    for (const testId of ['theme-density', 'theme-font', 'theme-mono']) {
      const select = screen.getByTestId(testId);
      const describedBy = select.getAttribute('aria-describedby') ?? '';
      expect(document.getElementById(describedBy)?.textContent, testId).toMatch(/mood, spacing/);
    }

    // The Appearance group explains the flip before it happens, because the
    // palette is re-based and only the accent survives.
    const mode = document.querySelector('[aria-labelledby] .theme-segment')?.parentElement;
    expect(mode?.getAttribute('aria-describedby')).toBeTruthy();
  });
});
