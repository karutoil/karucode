/* Public surface of the theme engine.
 *
 * Components import from here, never from the individual modules, so the
 * internal split (colour maths, presets, token resolution, storage) stays free
 * to change.
 */

export * from './types';
export {
  BUILTIN_THEMES,
  DEFAULT_THEME,
  completeColors,
  defaultColors,
  findBuiltin,
  modePreset,
} from './builtins';
export {
  contrastRatio,
  hexToRgb,
  isDarkColor,
  mix,
  normalizeHex,
  readableTextOn,
  relativeLuminance,
  rgbToHex,
  withAlpha,
} from './color';
export {
  DENSITY_SCALES,
  FONT_STACKS,
  MAX_ROUNDING,
  MIN_ROUNDING,
  MONO_STACKS,
  THEME_TOKEN_ENTRIES,
  buildTokenMap,
  clampRounding,
  resolvePalette,
  resolveTheme,
} from './tokens';
export { ROLE_PROPERTIES, resolveRoles } from './roles';
export {
  MAX_CUSTOM_CSS,
  MAX_CUSTOM_THEMES,
  MAX_IMPORT_BYTES,
  THEME_FORMAT,
  THEME_FORMAT_VERSION,
  THEME_STORAGE_KEY,
  ThemeFileError,
  clearPersistedState,
  exportThemeJSON,
  isValidThemeId,
  loadPersistedState,
  makeThemeId,
  parseThemeFile,
  sanitizeCustomCSS,
  sanitizeTheme,
  sanitizeThemeName,
  savePersistedState,
  themeFileSlug,
} from './storage';
export {
  activeTheme,
  activeThemeId,
  activeThemeName,
  applyTheme,
  autosaveError,
  baseThemeId,
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
  updateTheme,
} from './engine';