import { beforeEach, describe, expect, it } from 'vitest';
import { BUILTIN_THEMES, DEFAULT_THEME, modePreset } from './builtins';
import { contrastRatio, isDarkColor } from './color';
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
  setWorking,
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
    // Compare against the theme's own palette rather than a copy of it: the whole
    // point of this test is that the engine and the palette cannot drift apart.
    expect(token('--theme-bg')).toBe(DEFAULT_THEME.colors.bg);
    expect(token('--theme-accent')).toBe('#dfb680');
    expect(root().style.colorScheme).toBe('dark');
    expect(document.querySelector('meta[name="theme-color"]')?.getAttribute('content')).toBe(
      DEFAULT_THEME.colors.bg,
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
    expect(token('--theme-bg')).toBe('#f6f4f1');
    expect(token('--theme-text')).toBe('#1c1a17');
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
    // rounding 4 => sm = max(0, 4-4) = 0, md = 2, lg = 8 (DESIGN-SYSTEM.md §3.4)
    expect(token('--radius-sm')).toBe('0px');
    expect(token('--radius-md')).toBe('2px');
    expect(token('--radius-lg')).toBe('8px');
    expect(token('--font-ui')).toContain('Trebuchet');
    expect(token('--font-mono')).toContain('Courier New');
    expect(document.getElementById('kc-theme-custom-css')?.textContent).toContain('blur(6px)');
    expect(persisted()?.draft?.density).toBe('compact');
  });

  it('applies and persists the transcript view options', () => {
    initTheme();
    expect(root().dataset.toolStyle).toBe('rows');
    expect(root().dataset.grouping).toBe('turns');
    expect(root().dataset.working).toBe('open');
    expect(root().dataset.thinking).toBe('collapsed');

    setToolStyle('cards');
    setGrouping('none');
    setWorking('folded');
    setThinking('preview');

    expect(root().dataset.toolStyle).toBe('cards');
    expect(root().dataset.grouping).toBe('none');
    expect(root().dataset.working).toBe('folded');
    expect(root().dataset.thinking).toBe('preview');
    expect(persisted()?.draft).toMatchObject({
      toolStyle: 'cards',
      grouping: 'none',
      working: 'folded',
      thinking: 'preview',
    });
  });

  it('uses the new defaults for a stored built-in with no explicit view options', () => {
    // Only the selection id was stored; there is no user view choice to override.
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
    expect(root().dataset.toolStyle).toBe('rows');
    expect(root().dataset.grouping).toBe('turns');
    expect(root().dataset.working).toBe('open');
    expect(root().dataset.thinking).toBe('collapsed');
  });

  it('restores an explicitly stored cards/no-grouping preference', () => {
    const fullActivity = {
      ...getTheme('forest'),
      id: 'custom-full-activity',
      name: 'Full activity',
      toolStyle: 'cards' as const,
      grouping: 'none' as const,
    };
    localStorage.setItem(
      THEME_STORAGE_KEY,
      JSON.stringify({
        version: 1,
        selected: fullActivity.id,
        baseId: fullActivity.id,
        custom: [fullActivity],
        draft: null,
      }),
    );

    initTheme();
    expect(activeTheme.value.toolStyle).toBe('cards');
    expect(activeTheme.value.grouping).toBe('none');
    expect(root().dataset.toolStyle).toBe('cards');
    expect(root().dataset.grouping).toBe('none');
  });

  it('re-bases the palette when the mode flips but keeps the accent', () => {
    updateColors({ accent: '#00aa88' });
    setThemeMode('light');
    expect(root().dataset.mode).toBe('light');
    expect(activeTheme.value.mode).toBe('light');
    expect(activeTheme.value.colors.accent).toBe('#00aa88');
    expect(activeTheme.value.colors.bg).not.toBe('#08090b');
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

  it('reports the contrast of the applied palette, not of the raw hex pair', () => {
    /* The palette resolver nudges primary text until it clears 11:1 on every
     * surface, so `colors.text`/`colors.bg` can describe a pair that is not on
     * screen. The footer used to print that pair's ratio, which disagreed with
     * the drawer it sits in. */
    selectTheme('paper');
    updateColors({ text: '#e8e4dd' });
    const theme = activeTheme.value;
    expect(contrastRatio(theme.colors.text, theme.colors.bg)).toBeLessThan(4.5);
    expect(themeContrast.value).toBeGreaterThanOrEqual(11);
    expect(themeContrast.value).toBeCloseTo(contrastRatio(theme.palette.text, theme.palette.bg), 6);
  });

  it('still warns when the applied pair really is illegible', () => {
    // A hostile palette: the resolver can only walk toward an ink that is itself
    // the canvas, so this one stays broken and the footer has to say so.
    selectTheme('paper');
    updateColors({ bg: '#000000', text: '#000000' });
    expect(themeContrast.value).toBeLessThan(4.5);
  });
});

describe('palette and mode belong together', () => {
  /**
   * The palette `mode` must produce: the mode's own preset surfaces plus the two
   * colours that carry the theme's identity (the accent, and the syntax keyword
   * that follows it). `modePreset()` alone is the *stock* Daybreak/Ember palette.
   */
  function paletteFor(mode: 'dark' | 'light', accent: string) {
    return { ...modePreset(mode).colors, accent, syntaxKeyword: accent };
  }
  /** The floors `resolvePalette` promises, checked on whatever it just returned. */
  function expectLegible(id: string, mode: string): void {
    const theme = activeTheme.value;
    expect(theme.mode, id).toBe(mode as typeof theme.mode);
    // The canvas the palette paints has to belong to the mode it declares. This is
    // the invariant "Ember -> Light -> Reset colours" used to break: it restored
    // Ember's near-black surfaces while `data-mode` said light, and the drawer
    // (text, muted labels, borders, chips) fell to 6 of 92 text nodes below AA.
    expect(isDarkColor(theme.colors.bg), `${id}: canvas lightness vs mode`).toBe(mode === 'dark');
    const p = theme.palette;
    const surfaces = [p.bg, p.rail, p.sidebar, p.inspector, p.surface, p.raised, p.hover];
    for (const surface of surfaces) {
      expect(contrastRatio(p.text, surface), `${id}: text on ${surface}`).toBeGreaterThanOrEqual(11);
      expect(contrastRatio(p.textSecondary, surface), `${id}: text-secondary on ${surface}`)
        .toBeGreaterThanOrEqual(7);
      expect(contrastRatio(p.muted, surface), `${id}: muted on ${surface}`)
        .toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(p.faint, surface), `${id}: faint on ${surface}`)
        .toBeGreaterThanOrEqual(3);
    }
    for (const [label, ink] of [
      ['accent-text', p.accentText],
      ['warn', p.warn],
    ] as const) {
      expect(contrastRatio(ink, p.surface), `${id}: ${label} on surface`).toBeGreaterThanOrEqual(4.5);
    }
    for (const [label, ink, fill] of [
      ['success', p.success, p.successBg],
      ['danger', p.danger, p.dangerBg],
    ] as const) {
      expect(contrastRatio(ink, fill), `${id}: ${label}`).toBeGreaterThanOrEqual(4.5);
    }
    expect(contrastRatio(p.onAccent, p.accentText), `${id}: ink on the accent fill`)
      .toBeGreaterThanOrEqual(4.5);
  }

  it.each(BUILTIN_THEMES.map((theme) => theme.id))(
    '%s: flipping the mode keeps a legible palette for the new mode',
    (id) => {
      selectTheme(id);
      const before = activeTheme.value.mode;
      const next = before === 'dark' ? 'light' : 'dark';

      setThemeMode(next);

      expect(activeTheme.value.colors).toEqual(paletteFor(next, getTheme(id).colors.accent));
      // The accent is the identity colour and survives the flip.
      expect(activeTheme.value.colors.accent).toBe(getTheme(id).colors.accent);
      expectLegible(`${id} flipped to ${next}`, next);
    },
  );

  it.each(BUILTIN_THEMES.map((theme) => theme.id))(
    '%s: Reset colours after a flip restores that mode’s palette, not the base’s dark one',
    (id) => {
      selectTheme(id);
      const base = getTheme(id);
      const next = base.mode === 'dark' ? 'light' : 'dark';
      setThemeMode(next);
      updateColors({ bg: '#123456', surface: '#654321' });

      resetColors();

      expect(activeTheme.value.mode).toBe(next);
      expect(activeTheme.value.colors).toEqual(paletteFor(next, base.colors.accent));
      expect(activeTheme.value.colors.accent).toBe(base.colors.accent);
      expectLegible(`${id} reset in ${next}`, next);
      // The draft is still a draft: reset restores colours, not the save state.
      expect(draftEdited.value).toBe(true);
    },
  );

  it('resets to the base palette verbatim while the mode is unchanged', () => {
    // The mode-aware branch must not touch the ordinary case.
    selectTheme('forest');
    updateColors({ bg: '#000000', accent: '#ff0000' });
    resetColors();
    expect(activeTheme.value.colors).toEqual(getTheme('forest').colors);
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