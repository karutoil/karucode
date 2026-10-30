import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_THEME } from './builtins';
import {
  MAX_CUSTOM_CSS,
  THEME_STORAGE_KEY,
  ThemeFileError,
  exportThemeJSON,
  loadPersistedState,
  makeThemeId,
  parseThemeFile,
  sanitizeCustomCSS,
  sanitizeTheme,
  savePersistedState,
  themeFileSlug,
} from './storage';

function customTheme(name = 'Custom', id = 'custom-test'): Record<string, unknown> {
  return { ...DEFAULT_THEME, id, name, colors: { ...DEFAULT_THEME.colors } };
}

beforeEach(() => {
  localStorage.clear();
});

describe('sanitizeCustomCSS', () => {
  it('keeps normal CSS, strips closing style tags and caps the payload', () => {
    expect(sanitizeCustomCSS(':root { --accent: red; }')).toBe(':root { --accent: red; }');
    expect(sanitizeCustomCSS('.a { content: "</style><script>"; }')).toBe(
      '.a { content: "<script>"; }',
    );
    expect(sanitizeCustomCSS('x'.repeat(MAX_CUSTOM_CSS + 100))).toHaveLength(MAX_CUSTOM_CSS);
    expect(sanitizeCustomCSS(42)).toBe('');
  });
});

describe('sanitizeTheme', () => {
  it('accepts a valid stored theme', () => {
    const theme = sanitizeTheme(customTheme());
    expect(theme?.id).toBe('custom-test');
    expect(theme?.name).toBe('Custom');
    expect(theme?.colors.bg).toBe(DEFAULT_THEME.colors.bg);
  });

  it('rejects invalid shapes, ids, names and colours', () => {
    expect(sanitizeTheme(null)).toBeNull();
    expect(sanitizeTheme([])).toBeNull();
    expect(sanitizeTheme({ ...customTheme(), id: 'draft' })).toBeNull();
    expect(sanitizeTheme({ ...customTheme(), id: '../escape' })).toBeNull();
    expect(sanitizeTheme({ ...customTheme(), name: '   ' })).toBeNull();
    expect(sanitizeTheme({ ...customTheme(), colors: null })).toBeNull();
    expect(sanitizeTheme({ ...customTheme(), colors: { bg: 'not-a-colour' } })).toBeNull();
  });

  it('clamps the radius, defaults knobs and strips unknown colour keys', () => {
    const theme = sanitizeTheme({
      ...customTheme(),
      rounding: 90,
      density: 'enormous',
      mode: 'light',
      colors: { accent: '#ABC', bogus: '#000000' },
    });
    expect(theme?.rounding).toBe(22);
    expect(theme?.density).toBe('comfortable');
    expect(theme?.mode).toBe('light');
    expect(theme?.colors.accent).toBe('#aabbcc');
    expect((theme?.colors as unknown as Record<string, string>).bogus).toBeUndefined();
    // Missing colours are completed from the light defaults.
    expect(theme?.colors.bg).toBe('#f7f8fb');
  });

  it('accepts the transcript view options', () => {
    const theme = sanitizeTheme({
      ...customTheme(),
      toolStyle: 'rows',
      grouping: 'turns',
      thinking: 'expanded',
    });
    expect(theme?.toolStyle).toBe('rows');
    expect(theme?.grouping).toBe('turns');
    expect(theme?.thinking).toBe('expanded');
  });

  it('loads a theme saved before the view options existed', () => {
    // Every saved theme in every browser predates these three keys; dropping the
    // whole theme (or throwing) would take the user's palette with it.
    const legacy = { ...customTheme() };
    delete (legacy as Record<string, unknown>).toolStyle;
    delete (legacy as Record<string, unknown>).grouping;
    delete (legacy as Record<string, unknown>).thinking;

    const theme = sanitizeTheme(legacy);
    expect(theme).not.toBeNull();
    expect(theme?.toolStyle).toBe('cards');
    expect(theme?.grouping).toBe('none');
    expect(theme?.thinking).toBe('collapsed');
  });

  it('falls back on an unknown view option instead of rejecting the theme', () => {
    const theme = sanitizeTheme({
      ...customTheme(),
      toolStyle: 'holographic',
      grouping: 42,
      thinking: null,
    });
    expect(theme?.toolStyle).toBe('cards');
    expect(theme?.grouping).toBe('none');
    expect(theme?.thinking).toBe('collapsed');
  });
});

describe('import and export', () => {
  it('exports stable JSON and parses it back', () => {
    const json = exportThemeJSON({ ...DEFAULT_THEME, name: 'Exported', customCSS: '.x{}' });
    expect(json).toContain('"format": "karucode-theme"');
    expect(json).toContain('"name": "Exported"');

    const parsed = parseThemeFile(json);
    expect(parsed.name).toBe('Exported');
    expect(parsed.mode).toBe('dark');
    expect(parsed.customCSS).toBe('.x{}');
    expect(parsed.colors).toEqual(DEFAULT_THEME.colors);
    // Imports never reuse the exported id.
    expect(parsed.id).not.toBe(DEFAULT_THEME.id);
  });

  it('accepts partial themes and falls back to a default name', () => {
    const parsed = parseThemeFile(
      JSON.stringify({ mode: 'light', colors: { bg: '#ffffff', accent: '#123456' } }),
    );
    expect(parsed.name).toBe('Imported theme');
    expect(parsed.mode).toBe('light');
    expect(parsed.colors.bg).toBe('#ffffff');
    expect(parsed.colors.syntaxKeyword).toBe('#123456');
  });

  it('explains why a file is rejected', () => {
    const cases: Array<[string, RegExp]> = [
      ['', /empty/i],
      ['{not json', /valid JSON/i],
      ['[]', /not a theme/i],
      [JSON.stringify({ format: 'something-else', colors: {} }), /not a karucode theme/i],
      [JSON.stringify({ name: 'X', colors: { bg: 'blue' } }), /not a valid hex colour/i],
      [`{"name":"X","colors":{}}`.padEnd(200_000, ' '), /too large/i],
    ];
    for (const [text, pattern] of cases) {
      expect(() => parseThemeFile(text), text.slice(0, 40)).toThrow(ThemeFileError);
      expect(() => parseThemeFile(text), text.slice(0, 40)).toThrow(pattern);
    }
  });
});

describe('persisted state', () => {
  it('round-trips selection, custom themes and a draft', () => {
    const draft = { ...DEFAULT_THEME, id: 'draft', name: 'Untitled theme' };
    expect(
      savePersistedState({
        selected: 'ember',
        baseId: 'ember',
        custom: [customTheme() as never],
        draft,
      }),
    ).toBe(true);

    const state = loadPersistedState();
    expect(state?.selected).toBe('ember');
    expect(state?.custom).toHaveLength(1);
    expect(state?.custom[0]?.name).toBe('Custom');
    expect(state?.draft?.id).toBe('draft');
    expect(state?.version).toBe(1);
  });

  it('skips corrupt entries instead of losing the whole library', () => {
    localStorage.setItem(
      THEME_STORAGE_KEY,
      JSON.stringify({
        selected: 'custom-test',
        custom: [customTheme(), { name: 'broken' }, customTheme('Second', 'custom-second')],
      }),
    );
    const state = loadPersistedState();
    expect(state?.custom.map((theme) => theme.name)).toEqual(['Custom', 'Second']);
    expect(state?.selected).toBe('custom-test');
  });

  it('ignores stored themes whose id shadows a built-in', () => {
    localStorage.setItem(
      THEME_STORAGE_KEY,
      JSON.stringify({
        selected: 'ember',
        custom: [customTheme('Impostor', 'ember'), customTheme('Real', 'custom-real')],
      }),
    );
    const state = loadPersistedState();
    expect(state?.custom.map((theme) => theme.name)).toEqual(['Real']);
  });

  it('returns null for unreadable storage', () => {
    localStorage.setItem(THEME_STORAGE_KEY, '{oops');
    expect(loadPersistedState()).toBeNull();
    localStorage.removeItem(THEME_STORAGE_KEY);
    expect(loadPersistedState()).toBeNull();
  });

  it('reports a failed write instead of throwing', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('quota');
    });
    expect(
      savePersistedState({ selected: 'ember', baseId: 'ember', custom: [], draft: null }),
    ).toBe(false);
  });
});

describe('helpers', () => {
  it('creates unique custom ids', () => {
    const ids = new Set(Array.from({ length: 20 }, () => makeThemeId()));
    expect(ids.size).toBe(20);
    for (const id of ids) expect(id).toMatch(/^custom-[a-z0-9]+-[a-z0-9]+$/);
  });

  it('slugs names for downloads', () => {
    expect(themeFileSlug('My Custom Theme!')).toBe('my-custom-theme');
    expect(themeFileSlug('***')).toBe('theme');
  });
});