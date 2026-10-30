import { beforeEach, describe, expect, it } from 'vitest';
import { DEFAULT_THEME } from './builtins';
import {
  activeTheme,
  activeThemeId,
  activeThemeName,
  autosaveError,
  customThemes,
  deleteCustomTheme,
  draftEdited,
  getTheme,
  importTheme,
  initTheme,
  isCustomThemeId,
  resetColors,
  resetTheme,
  saveThemeAsNew,
  selectTheme,
  setCustomCSS,
  setDensity,
  setFont,
  setGrouping,
  setMono,
  setRounding,
  setThemeMode,
  setThinking,
  setToolStyle,
  themeContrast,
  themeLibrary,
  updateColors,
} from './engine';
import {
  THEME_STORAGE_KEY,
  ThemeFileError,
  exportThemeJSON,
  type PersistedThemeState,
} from './storage';

const root = (): HTMLElement => document.documentElement;
const token = (property: string): string => root().style.getPropertyValue(property);
const persisted = (): PersistedThemeState | null => {
  const raw = localStorage.getItem(THEME_STORAGE_KEY);
  return raw ? (JSON.parse(raw) as PersistedThemeState) : null;
};

beforeEach(() => {
  localStorage.clear();
  resetTheme();
});

describe('initTheme', () => {
  it('paints Ember and sets the data attributes on <html>', () => {
    initTheme();
    expect(root().dataset.theme).toBe('ember');
    expect(root().dataset.mode).toBe('dark');
    expect(root().dataset.density).toBe('comfortable');
    expect(token('--theme-bg')).toBe('#111213');
    expect(token('--theme-accent')).toBe('#dfb680');
    expect(root().style.colorScheme).toBe('dark');
    expect(document.querySelector('meta[name="theme-color"]')?.getAttribute('content')).toBe(
      '#111213',
    );
  });

  it('restores the persisted saved theme', () => {
    selectTheme('forest');
    expect(root().dataset.theme).toBe('forest');
    initTheme();
    expect(root().dataset.theme).toBe('forest');
    expect(token('--theme-bg')).toBe(getTheme('forest').colors.bg);
  });

  it('restores a persisted draft and labels it unsaved', () => {
    updateColors({ bg: '#0a0b0c' });
    expect(draftEdited.value).toBe(true);
    initTheme();
    expect(root().dataset.theme).toBe('draft');
    expect(token('--theme-bg')).toBe('#0a0b0c');
    expect(draftEdited.value).toBe(true);
    expect(activeThemeName.value).toBe('Unsaved theme');
  });

  it('ignores an unknown saved selection', () => {
    localStorage.setItem(
      THEME_STORAGE_KEY,
      JSON.stringify({ version: 1, selected: 'gone', baseId: 'gone', custom: [], draft: null }),
    );
    initTheme();
    expect(root().dataset.theme).toBe('ember');
  });
});

describe('selecting a built-in', () => {
  it('repaints every token and persists the selection', () => {
    expect(selectTheme('daybreak')).toBe(true);
    expect(root().dataset.theme).toBe('daybreak');
    expect(root().dataset.mode).toBe('light');
    expect(root().style.colorScheme).toBe('light');
    expect(token('--theme-bg')).toBe('#f7f8fb');
    expect(token('--theme-text')).toBe('#1b283a');
    expect(draftEdited.value).toBe(false);
    expect(persisted()?.selected).toBe('daybreak');
  });

  it('refuses unknown ids and unknown drafts', () => {
    expect(selectTheme('nope')).toBe(false);
    expect(selectTheme('draft')).toBe(false);
    expect(root().dataset.theme).toBe('ember');
  });

  it('lists built-ins then custom themes', () => {
    expect(themeLibrary.value).toHaveLength(8);
    saveThemeAsNew('Mine');
    expect(themeLibrary.value).toHaveLength(9);
    expect(themeLibrary.value[8]?.name).toBe('Mine');
  });
});

describe('editing a draft', () => {
  it('applies colours live, marks the draft and persists it', () => {
    updateColors({ accent: '#ff5500', bg: '#101010' });
    expect(draftEdited.value).toBe(true);
    expect(activeThemeId.value).toBe('draft');
    expect(root().dataset.theme).toBe('draft');
    expect(token('--theme-accent')).toBe('#ff5500');
    expect(token('--theme-bg')).toBe('#101010');
    // Derived tokens follow the accent.
    expect(token('--theme-accent-hi')).not.toBe('#ff5500');
    expect(persisted()?.draft?.colors.accent).toBe('#ff5500');
  });

  it('applies density, fonts, radius and custom CSS', () => {
    setDensity('compact');
    setFont('rounded');
    setMono('classic');
    setRounding(4);
    setCustomCSS('.project-rail { backdrop-filter: blur(6px); }');

    expect(root().dataset.density).toBe('compact');
    expect(token('--fs-body')).toBe('12px');
    expect(token('--radius')).toBe('4px');
    expect(token('--radius-sm')).toBe('1px');
    expect(token('--font-ui')).toContain('Trebuchet');
    expect(token('--font-mono')).toContain('Courier New');
    expect(document.getElementById('kc-theme-custom-css')?.textContent).toContain('blur(6px)');
    expect(persisted()?.draft?.density).toBe('compact');
  });

  it('applies and persists the transcript view options', () => {
    initTheme();
    expect(root().dataset.toolStyle).toBe('cards');
    expect(root().dataset.grouping).toBe('none');
    expect(root().dataset.thinking).toBe('collapsed');

    setToolStyle('rows');
    setGrouping('turns');
    setThinking('preview');

    expect(root().dataset.toolStyle).toBe('rows');
    expect(root().dataset.grouping).toBe('turns');
    expect(root().dataset.thinking).toBe('preview');
    expect(persisted()?.draft).toMatchObject({
      toolStyle: 'rows',
      grouping: 'turns',
      thinking: 'preview',
    });
  });

  it('keeps a theme saved before the view options working', () => {
    // A stored selection with no view options must load and paint the defaults.
    localStorage.setItem(
      THEME_STORAGE_KEY,
      JSON.stringify({
        version: 1,
        selected: 'forest',
        baseId: 'forest',
        custom: [],
        draft: null,
      }),
    );
    initTheme();
    expect(root().dataset.theme).toBe('forest');
    expect(root().dataset.toolStyle).toBe('cards');
    expect(root().dataset.grouping).toBe('none');
    expect(root().dataset.thinking).toBe('collapsed');
  });

  it('re-bases the palette when the mode flips but keeps the accent', () => {
    updateColors({ accent: '#00aa88' });
    setThemeMode('light');
    expect(root().dataset.mode).toBe('light');
    expect(activeTheme.value.mode).toBe('light');
    expect(activeTheme.value.colors.accent).toBe('#00aa88');
    expect(activeTheme.value.colors.bg).not.toBe('#111213');
    expect(root().style.colorScheme).toBe('light');
  });

  it('resets colours back to the base theme', () => {
    selectTheme('forest');
    updateColors({ bg: '#000000', accent: '#ff0000' });
    resetColors();
    expect(activeTheme.value.colors).toEqual(getTheme('forest').colors);
    expect(activeTheme.value.name).toBe('Untitled theme');
  });

  it('reports the text/background contrast for the studio footer', () => {
    expect(themeContrast.value).toBeGreaterThan(4.5);
  });
});

describe('saving and deleting custom themes', () => {
  it('saves the draft as a new theme and selects it', () => {
    updateColors({ accent: '#7c5cff' });
    const saved = saveThemeAsNew('Midnight');
    expect(saved).not.toBeNull();
    expect(saved?.name).toBe('Midnight');
    expect(customThemes.value).toHaveLength(1);
    expect(draftEdited.value).toBe(false);
    expect(activeThemeId.value).toBe(saved?.id);
    expect(activeThemeName.value).toBe('Midnight');
    expect(root().dataset.theme).toBe(saved?.id);
    expect(isCustomThemeId(saved!.id)).toBe(true);
    expect(persisted()?.selected).toBe(saved?.id);
    expect(persisted()?.draft).toBeNull();
  });

  it('rejects a blank name and disambiguates duplicates', () => {
    expect(saveThemeAsNew('   ')).toBeNull();
    saveThemeAsNew('Mine');
    const duplicate = saveThemeAsNew('Mine');
    expect(duplicate?.name).toBe('Mine (2)');
  });

  it('persists a custom theme across a reload', () => {
    updateColors({ bg: '#050607' });
    const saved = saveThemeAsNew('Night');
    initTheme();
    expect(activeThemeId.value).toBe(saved?.id);
    expect(token('--theme-bg')).toBe('#050607');
    expect(customThemes.value).toHaveLength(1);
  });

  it('deletes a custom theme and falls back to Ember', () => {
    const saved = saveThemeAsNew('Temporary');
    expect(deleteCustomTheme(saved!.id)).toBe(true);
    expect(customThemes.value).toHaveLength(0);
    expect(activeThemeId.value).toBe('ember');
    expect(root().dataset.theme).toBe('ember');
    expect(persisted()?.custom).toEqual([]);
  });

  it('cannot delete built-ins or unknown ids', () => {
    expect(deleteCustomTheme('ember')).toBe(false);
    expect(deleteCustomTheme('nope')).toBe(false);
  });
});

describe('import and export', () => {
  it('imports a theme file, adds it to the library and applies it', () => {
    const json = exportThemeJSON({ ...DEFAULT_THEME, name: 'Imported', mode: 'light' });
    const theme = importTheme(json);
    expect(theme.name).toBe('Imported');
    expect(customThemes.value.map((entry) => entry.name)).toContain('Imported');
    expect(activeThemeId.value).toBe(theme.id);
    expect(root().dataset.mode).toBe('light');
    expect(persisted()?.custom).toHaveLength(1);
  });

  it('does not mutate the library when a file is invalid', () => {
    expect(() => importTheme('{}')).toThrow(ThemeFileError);
    expect(customThemes.value).toHaveLength(0);
    expect(activeThemeId.value).toBe('ember');
  });

  it('exports the applied draft, not the saved base', () => {
    updateColors({ accent: '#123456' });
    const json = JSON.parse(exportThemeJSON(activeTheme.value)) as {
      colors: { accent: string };
    };
    expect(json.colors.accent).toBe('#123456');
  });
});

describe('storage failures', () => {
  it('keeps working in memory and surfaces the autosave error', () => {
    const original = Storage.prototype.setItem;
    Storage.prototype.setItem = () => {
      throw new Error('quota');
    };
    try {
      updateColors({ bg: '#222222' });
      expect(token('--theme-bg')).toBe('#222222');
      expect(autosaveError.value).toMatch(/blocked saving/i);
    } finally {
      Storage.prototype.setItem = original;
    }
  });
});

describe('resetTheme', () => {
  it('clears the library, the draft and the persisted key', () => {
    saveThemeAsNew('Gone');
    resetTheme();
    expect(customThemes.value).toHaveLength(0);
    expect(draftEdited.value).toBe(false);
    expect(root().dataset.theme).toBe('ember');
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBeNull();
  });
});