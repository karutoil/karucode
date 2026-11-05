/* Theme Studio drawer.
 *
 * Mounted once by `app.tsx`; visibility is driven by `store.themeStudioOpen`,
 * opened via `actions.openThemeStudio()` from the shell header/rail and the
 * `Ctrl/Cmd+Shift+T` shortcut (owned by `app.tsx`, next to the other global
 * shortcuts, so the drawer does not race a second listener).
 *
 * Design reference: `karucode-v3-themes.html`. Unlike the mock, nothing here
 * uses `window.prompt`/`window.confirm`: naming is an inline form and deleting
 * is an inline confirmation, so the flow stays keyboard- and screen-reader-
 * friendly (and testable).
 */

import type { JSX } from 'preact';
import { createPortal } from 'preact/compat';
import { useEffect, useId, useRef, useState } from 'preact/hooks';
import { useFocusTrap } from '../lib/dom';
import * as actions from '../state/actions';
import * as store from '../state/store';
import {
  MAX_CUSTOM_CSS,
  MAX_IMPORT_BYTES,
  MAX_ROUNDING,
  MIN_ROUNDING,
  ROLE_PROPERTIES,
  THEME_COLOR_GROUPS,
  THEME_DENSITIES,
  THEME_FONTS,
  THEME_FONT_LABELS,
  THEME_GROUPING_CHOICES,
  THEME_MONO_FONTS,
  THEME_MONO_LABELS,
  THEME_THINKING_CHOICES,
  THEME_TOKEN_ENTRIES,
  THEME_TOOL_STYLE_CHOICES,
  ThemeFileError,
  activeTheme,
  autosaveError,
  buildTokenMap,
  customThemes,
  deleteCustomTheme,
  draftEdited,
  exportThemeJSON,
  importTheme,
  normalizeHex,
  resetColors,
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
  themeFileSlug,
  themeLibrary,
  updateColors,
  type Theme,
  type ThemeChoice,
  type ThemeColorKey,
  type ThemeColorsPatch,
  type ThemeDensity,
  type ThemeFont,
  type ThemeGrouping,
  type ThemeMono,
  type ThemeThinking,
  type ThemeToolStyle,
} from '../theme';
import {
  AlertIcon,
  CheckIcon,
  ChevronDownIcon,
  CloseIcon,
  CopyIcon,
  DownloadIcon,
  PaletteIcon,
  PlusIcon,
  RefreshIcon,
  TrashIcon,
  UploadIcon,
} from './icons';

const DENSITY_LABELS: Record<ThemeDensity, string> = {
  compact: 'Compact',
  comfortable: 'Comfortable',
  spacious: 'Spacious',
};

const MODE_LABELS = { dark: 'Dark', light: 'Light' } as const;

/**
 * A segmented control for a transcript option. The hint below the row explains
 * what the selected value will do to the conversation, because "Turns" on its own
 * does not say what gets folded.
 */
function TranscriptChoice<T extends string>({
  label,
  value,
  choices,
  onChange,
  testId,
  describedBy,
}: {
  label: string;
  value: T;
  choices: readonly ThemeChoice<T>[];
  onChange: (next: T) => void;
  testId: string;
  describedBy: string;
}): JSX.Element {
  const labelId = useId();
  const selected = choices.find((choice) => choice.value === value);
  return (
    <div class="theme-field">
      <span class="theme-field-label" id={labelId}>
        {label}
      </span>
      <div
        class="theme-segments"
        role="group"
        aria-labelledby={labelId}
        aria-describedby={describedBy}
        data-testid={testId}
      >
        {choices.map((choice) => (
          <button
            type="button"
            key={choice.value}
            class={`theme-segment${choice.value === value ? ' selected' : ''}`}
            aria-pressed={choice.value === value}
            title={choice.hint}
            data-testid={`${testId}-${choice.value}`}
            onClick={() => onChange(choice.value)}
          >
            {choice.label}
          </button>
        ))}
      </div>
      <p class="theme-field-hint" id={describedBy}>
        {selected?.hint ?? ''}
      </p>
    </div>
  );
}

/** A miniature of the real transcript, repainted by these very options. */
function TranscriptPreview(): JSX.Element {
  const theme = activeTheme.value;
  return (
    <div class="theme-preview" data-testid="theme-transcript-preview" aria-hidden="true">
      <p class="theme-preview-narration" data-on={theme.grouping === 'turns' ? 'folded' : 'shown'}>
        Reading the transcript layout…
      </p>
      <div class="theme-preview-row" hidden={theme.toolStyle === 'cards'}>
        <span class="theme-preview-dot" />
        <span class="theme-preview-name">read_file</span>
        <span class="theme-preview-args">src/theme/types.ts</span>
        <span class="theme-preview-meta">12ms</span>
      </div>
      <div class="theme-preview-card" hidden={theme.toolStyle === 'rows'}>
        <span class="theme-preview-card-head">
          <span class="theme-preview-dot" />
          <span class="theme-preview-name">read_file</span>
          <span class="theme-preview-args">src/theme/types.ts</span>
        </span>
        <span class="theme-preview-card-body">export type ThemeToolStyle = …</span>
      </div>
      <p class={`theme-preview-thought${theme.thinking === 'collapsed' ? ' is-collapsed' : ''}`}>
        Thought{theme.thinking === 'preview' ? ' · The view options belong on the theme.' : ''}
      </p>
    </div>
  );
}

/** Section copy is the mock's own wording; the engine keeps the canonical data. */
const COLOR_GROUP_COPY: Record<string, { title: string; description: string }> = {
  backgrounds: {
    title: 'Backgrounds',
    description: 'Make the workspace yours, from the project rail to the editor.',
  },
  accents: {
    title: 'Typography & accents',
    description: 'Click a swatch or enter a hex code. Changes appear instantly.',
  },
  syntax: {
    title: 'Syntax highlighting',
    description: 'Fine-tune code colours independently of the application palette.',
  },
};

function groupCopy(group: { id: string; title: string; description: string }): {
  title: string;
  description: string;
} {
  return COLOR_GROUP_COPY[group.id] ?? { title: group.title, description: group.description };
}

/** `Blob.text()` is missing in jsdom and older Safari; fall back to FileReader. */
function readFileText(file: File): Promise<string> {
  if (typeof file.text === 'function') return file.text();
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(typeof reader.result === 'string' ? reader.result : '');
    reader.onerror = () => reject(reader.error ?? new Error('Could not read the file.'));
    reader.readAsText(file);
  });
}

function presetStyle(theme: Theme): string {
  return [
    `--preset-bg:${theme.colors.bg}`,
    `--preset-rail:${theme.colors.rail}`,
    `--preset-sidebar:${theme.colors.sidebar}`,
    `--preset-surface:${theme.colors.surface}`,
    `--preset-text:${theme.colors.text}`,
    `--preset-muted:${theme.colors.muted}`,
    `--preset-accent:${theme.colors.accent}`,
  ].join(';');
}

/** After a transient form disappears, keep focus inside the dialog. */
function refocusSaveButton(panel: HTMLElement | null): void {
  window.setTimeout(() => {
    panel?.querySelector<HTMLButtonElement>('[data-testid="theme-save-as-new"]')?.focus();
  }, 0);
}

export function ThemeStudio(): JSX.Element | null {
  const open = store.themeStudioOpen.value;
  const theme = activeTheme.value;
  const library = themeLibrary.value;
  const dirty = draftEdited.value;
  const contrast = themeContrast.value;
  const saveError = autosaveError.value;
  const customs = customThemes.value;

  const [naming, setNaming] = useState(false);
  const [nameValue, setNameValue] = useState('');
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [hexEdits, setHexEdits] = useState<Record<string, string>>({});

  const panelRef = useRef<HTMLElement>(null);
  const nameRef = useRef<HTMLInputElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);

  const titleId = useId();
  const subtitleId = useId();
  const nameLabelId = useId();
  const deleteLabelId = useId();
  const radiusId = useId();
  const interfaceDescId = useId();
  const modeHintId = useId();

  useFocusTrap(panelRef, open, actions.closeThemeStudio);

  // The focus trap handles Escape for events that originate inside the drawer.
  // Keep a window-level fallback so Escape still closes the drawer when focus is
  // on <body> (or an event is dispatched at the window, as tests do).
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== 'Escape') return;
      event.stopPropagation();
      actions.closeThemeStudio();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    setNaming(false);
    setNameValue('');
    setConfirmDelete(false);
    setHexEdits({});
  }, [open]);

  useEffect(() => {
    if (!naming) return;
    const timer = window.setTimeout(() => nameRef.current?.focus(), 0);
    return () => window.clearTimeout(timer);
  }, [naming]);

  useEffect(() => {
    if (!confirmDelete) return;
    const timer = window.setTimeout(() => confirmRef.current?.focus(), 0);
    return () => window.clearTimeout(timer);
  }, [confirmDelete]);

  if (!open) return null;

  const isCustom = customs.some((entry) => entry.id === theme.id);
  const activeLabel = dirty ? 'Unsaved theme' : theme.name;

  const openNaming = (): void => {
    setNaming(true);
    setConfirmDelete(false);
    setNameValue(dirty ? 'My custom theme' : `${theme.name} copy`);
  };

  const cancelNaming = (): void => {
    setNaming(false);
    setNameValue('');
    refocusSaveButton(panelRef.current);
  };

  const submitName = (event: Event): void => {
    event.preventDefault();
    const saved = saveThemeAsNew(nameValue);
    if (!saved) {
      store.pushToast('Enter a theme name first.', 'error');
      nameRef.current?.focus();
      return;
    }
    setNaming(false);
    setNameValue('');
    refocusSaveButton(panelRef.current);
    store.pushToast(`Saved “${saved.name}” to your themes.`, 'success');
  };

  const requestDelete = (): void => {
    if (!isCustom) return;
    setNaming(false);
    setConfirmDelete(true);
  };

  const confirmDeleteNow = (): void => {
    const deletedName = theme.name;
    if (deleteCustomTheme(theme.id)) {
      store.pushToast(`Deleted “${deletedName}”.`, 'success');
    }
    setConfirmDelete(false);
    refocusSaveButton(panelRef.current);
  };

  const onImportFile = async (event: JSX.TargetedEvent<HTMLInputElement, Event>): Promise<void> => {
    const input = event.currentTarget;
    const file = input.files?.[0];
    // Allow picking the same file again after a failed import.
    input.value = '';
    if (!file) return;
    if (file.size > MAX_IMPORT_BYTES) {
      store.pushToast('That theme file is too large (limit 150 KB).', 'error');
      return;
    }
    try {
      const imported = importTheme(await readFileText(file));
      store.pushToast(`Imported “${imported.name}”.`, 'success');
    } catch (error) {
      const message =
        error instanceof ThemeFileError || error instanceof Error
          ? error.message
          : 'Could not import that theme.';
      store.pushToast(message, 'error');
    }
  };

  const onExport = (): void => {
    try {
      if (typeof URL.createObjectURL !== 'function') throw new Error('No object URLs');
      const blob = new Blob([exportThemeJSON(theme)], { type: 'application/json' });
      const href = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = href;
      anchor.download = `karucode-${themeFileSlug(theme.name)}.theme.json`;
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      window.setTimeout(() => URL.revokeObjectURL(href), 3000);
      store.pushToast('Theme JSON exported.', 'success');
    } catch {
      store.pushToast('This browser blocked the download.', 'error');
    }
  };

  const copyVariables = async (): Promise<void> => {
    // Emit the properties the app actually reads, with the values it actually
    // paints. The old block invented a `--kc-<colour>` name per palette entry,
    // and nothing in the stylesheet consumes a `--kc-*` property: pasting it
    // changed nothing. These are the real `--theme-*` tokens plus the fidelity
    // roles derived from them (`resolveRoles`), i.e. exactly what `applyTheme`
    // writes on <html>.
    const tokens = buildTokenMap(theme);
    const variables = [
      ...THEME_TOKEN_ENTRIES.map(([, property]) => property),
      ...ROLE_PROPERTIES,
    ]
      .filter((property) => tokens[property] !== undefined)
      // applyTheme writes these properties inline on <html>; a normal :root
      // declaration loses that cascade contest even though the editor is later.
      .map((property) => `  ${property}: ${tokens[property]} !important;`)
      .join('\n');
    const code = `/* !important lets Custom CSS replace the engine's inline token values. */\n/* ${theme.name} — ${theme.mode} */\n:root {\n${variables}\n}\n${theme.customCSS}`;
    try {
      await navigator.clipboard.writeText(code);
      store.pushToast('Theme variables copied.', 'success');
    } catch {
      store.pushToast('Clipboard is unavailable in this browser.', 'error');
    }
  };

  const onHexInput = (key: ThemeColorKey, raw: string): void => {
    setHexEdits((edits) => ({ ...edits, [key]: raw }));
    const normalized = normalizeHex(raw);
    if (normalized) updateColors({ [key]: normalized } as ThemeColorsPatch);
  };

  const onHexBlur = (key: ThemeColorKey): void => {
    setHexEdits((edits) => {
      const next = { ...edits };
      delete next[key];
      return next;
    });
  };

  const layer = (
    <div class="theme-studio-layer">
      <div class="theme-shade" aria-hidden="true" onClick={actions.closeThemeStudio} />
      <aside
        class="theme-studio"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={subtitleId}
        ref={panelRef}
        data-testid="theme-studio"
      >
        <header class="theme-studio-top">
          <div class="theme-studio-head">
            <div class="theme-top-eyebrow">
              <PaletteIcon size={14} />
              Personalization
              <span class="theme-head-badge">Live preview</span>
            </div>
            <h2 class="theme-studio-title" id={titleId}>
              Theme studio
            </h2>
            <p class="theme-studio-subtitle" id={subtitleId}>
              Make the whole workspace feel like yours.
            </p>
          </div>
          <button
            type="button"
            class="theme-close"
            onClick={actions.closeThemeStudio}
            aria-label="Close theme studio"
          >
            <CloseIcon />
          </button>
        </header>

        <div class="theme-studio-scroll">
          <section class="theme-section" aria-labelledby={`${titleId}-library`}>
            <div class="theme-section-title">
              <h3 id={`${titleId}-library`}>Theme library</h3>
              <button type="button" class="theme-mini-action" onClick={openNaming}>
                <PlusIcon size={13} />
                New theme
              </button>
            </div>
            <p class="theme-description">
              Start from a look you love. Every part of the UI can be customized.
            </p>

            {naming ? (
              <form class="theme-inline-create" onSubmit={submitName}>
                <label class="sr-only" for={nameLabelId} id={`${nameLabelId}-label`}>
                  Theme name
                </label>
                <input
                  ref={nameRef}
                  id={nameLabelId}
                  class="theme-name-input"
                  type="text"
                  value={nameValue}
                  maxLength={50}
                  placeholder="My custom theme"
                  autocomplete="off"
                  aria-describedby={`${nameLabelId}-hint`}
                  data-testid="theme-name-input"
                  onInput={(event) => setNameValue(event.currentTarget.value)}
                />
                <button type="submit" class="theme-btn primary" data-testid="theme-save-confirm">
                  <CheckIcon size={14} />
                  Save theme
                </button>
                <button type="button" class="theme-btn" onClick={cancelNaming}>
                  Cancel
                </button>
                <span class="sr-only" id={`${nameLabelId}-hint`}>
                  Saves the current colours and interface settings as a new custom theme.
                </span>
              </form>
            ) : null}

            <ul class="theme-gallery" role="list">
              {library.map((entry) => {
                const selected = !dirty && entry.id === theme.id;
                const custom = customs.some((candidate) => candidate.id === entry.id);
                const modeLabel = MODE_LABELS[entry.mode];
                return (
                  <li key={entry.id}>
                    <button
                      type="button"
                      class={`theme-preset${selected ? ' selected' : ''}`}
                      style={presetStyle(entry)}
                      aria-pressed={selected}
                      title={entry.description ?? entry.name}
                      data-testid={`theme-preset-${entry.id}`}
                      onClick={() => {
                        setNaming(false);
                        selectTheme(entry.id);
                      }}
                    >
                      <span class="theme-preset-demo" aria-hidden="true">
                        <span class="theme-mini-rail" />
                        <span class="theme-mini-sidebar">
                          <i />
                          <i />
                          <i />
                        </span>
                        <span class="theme-mini-main">
                          <i />
                          <i />
                          <i />
                        </span>
                        <span class="theme-mini-accent" />
                      </span>
                      <span class="theme-preset-meta">
                        <span class="theme-preset-name">{entry.name}</span>
                        <span class="theme-preset-mode">
                          {modeLabel}
                          {custom ? ' · Custom' : ''}
                        </span>
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          </section>

          {THEME_COLOR_GROUPS.map((group) => {
            const copy = groupCopy(group);
            const descriptionId = `${titleId}-${group.id}-desc`;
            return (
              <section class="theme-section" key={group.id} aria-labelledby={`${titleId}-${group.id}`}>
                <div class="theme-section-title">
                  <h3 id={`${titleId}-${group.id}`}>{copy.title}</h3>
                  {group.id === 'backgrounds' ? (
                    <button type="button" class="theme-mini-action" onClick={resetColors}>
                      <RefreshIcon size={12} />
                      Reset colours
                    </button>
                  ) : null}
                </div>
                <p class="theme-description" id={descriptionId}>
                  {copy.description}
                </p>
                <div class="theme-colors">
                  {group.fields.map((field) => {
                    const edit = hexEdits[field.key];
                    const value = theme.colors[field.key];
                    const shown = edit ?? value;
                    const invalid = edit !== undefined && normalizeHex(edit) === null;
                    const inputId = `${titleId}-${field.key}`;
                    return (
                      <div class="theme-color-cell" key={field.key}>
                        <span class="theme-color-swatch" style={{ background: value }}>
                          <input
                            id={inputId}
                            type="color"
                            value={value}
                            aria-label={`${field.label} colour picker`}
                            aria-describedby={descriptionId}
                            data-testid={`theme-color-${field.key}`}
                            onInput={(event) =>
                              updateColors({
                                [field.key]: event.currentTarget.value,
                              } as ThemeColorsPatch)
                            }
                          />
                        </span>
                        <span class="theme-color-info">
                          <label class="theme-color-name" for={inputId}>
                            {field.label}
                          </label>
                          <input
                            class={`theme-color-hex${invalid ? ' invalid' : ''}`}
                            type="text"
                            value={shown.toUpperCase()}
                            spellcheck={false}
                            maxLength={7}
                            aria-label={`${field.label} hex value`}
                            aria-invalid={invalid}
                            aria-describedby={descriptionId}
                            data-testid={`theme-hex-${field.key}`}
                            onInput={(event) => onHexInput(field.key, event.currentTarget.value)}
                            onBlur={() => onHexBlur(field.key)}
                          />
                        </span>
                      </div>
                    );
                  })}
                </div>
              </section>
            );
          })}

          <section class="theme-section" aria-labelledby={`${titleId}-interface`}>
            <div class="theme-section-title">
              <h3 id={`${titleId}-interface`}>Interface</h3>
            </div>
            <p class="theme-description" id={interfaceDescId}>
              Control the mood, spacing and typography across the entire workspace.
            </p>
            <div class="theme-setting-grid">
              <div class="theme-field">
                <span class="theme-field-label" id={`${titleId}-mode`}>
                  Appearance
                </span>
                <div
                  class="theme-segments"
                  role="group"
                  aria-labelledby={`${titleId}-mode`}
                  aria-describedby={modeHintId}
                >
                  {(['dark', 'light'] as const).map((mode) => (
                    <button
                      type="button"
                      key={mode}
                      class={`theme-segment${theme.mode === mode ? ' selected' : ''}`}
                      aria-pressed={theme.mode === mode}
                      data-testid={`theme-mode-${mode}`}
                      onClick={() => setThemeMode(mode)}
                    >
                      {MODE_LABELS[mode]}
                    </button>
                  ))}
                </div>
                {/* Says what the flip does *before* it happens: the palette is
                    re-based to the new mode and the accent survives, which is the
                    contract `setThemeMode` and `resetColors` share. */}
                <p class="theme-field-hint" id={modeHintId}>
                  Re-bases the surfaces to that mode and keeps your accent.
                </p>
              </div>

              <label class="theme-field">
                <span class="theme-field-label">Density</span>
                <select
                  value={theme.density}
                  aria-describedby={interfaceDescId}
                  data-testid="theme-density"
                  onChange={(event) => setDensity(event.currentTarget.value as ThemeDensity)}
                >
                  {THEME_DENSITIES.map((density) => (
                    <option value={density} key={density}>
                      {DENSITY_LABELS[density]}
                    </option>
                  ))}
                </select>
              </label>

              <label class="theme-field">
                <span class="theme-field-label">Interface font</span>
                <select
                  value={theme.font}
                  aria-describedby={interfaceDescId}
                  data-testid="theme-font"
                  onChange={(event) => setFont(event.currentTarget.value as ThemeFont)}
                >
                  {THEME_FONTS.map((font) => (
                    <option value={font} key={font}>
                      {THEME_FONT_LABELS[font]}
                    </option>
                  ))}
                </select>
              </label>

              <label class="theme-field">
                <span class="theme-field-label">Code font</span>
                <select
                  value={theme.mono}
                  aria-describedby={interfaceDescId}
                  data-testid="theme-mono"
                  onChange={(event) => setMono(event.currentTarget.value as ThemeMono)}
                >
                  {THEME_MONO_FONTS.map((mono) => (
                    <option value={mono} key={mono}>
                      {THEME_MONO_LABELS[mono]}
                    </option>
                  ))}
                </select>
              </label>
            </div>

            <div class="theme-range-row">
              <label class="theme-range-label" for={radiusId}>
                Corner radius
              </label>
              <input
                id={radiusId}
                type="range"
                min={MIN_ROUNDING}
                max={MAX_ROUNDING}
                step={1}
                value={theme.rounding}
                data-testid="theme-radius"
                onInput={(event) => setRounding(Number(event.currentTarget.value))}
              />
              <output class="theme-range-value" for={radiusId}>
                {theme.rounding}px
              </output>
            </div>
          </section>

          <section class="theme-section" aria-labelledby={`${titleId}-transcript`}>
            <div class="theme-section-title">
              <h3 id={`${titleId}-transcript`}>Transcript</h3>
            </div>
            <p class="theme-description">
              Decide how much of the agent’s work the conversation shows at once. Long sessions
              stay readable without hiding anything: every folded step opens in a click.
            </p>

            <TranscriptPreview />

            <div class="theme-setting-grid theme-transcript-grid">
              <TranscriptChoice
                label="Tool calls"
                value={theme.toolStyle}
                choices={THEME_TOOL_STYLE_CHOICES}
                onChange={(next) => setToolStyle(next as ThemeToolStyle)}
                testId="theme-tool-style"
                describedBy={`${titleId}-tool-style-hint`}
              />
              <TranscriptChoice
                label="Grouping"
                value={theme.grouping}
                choices={THEME_GROUPING_CHOICES}
                onChange={(next) => setGrouping(next as ThemeGrouping)}
                testId="theme-grouping"
                describedBy={`${titleId}-grouping-hint`}
              />
            </div>

            <TranscriptChoice
              label="Reasoning"
              value={theme.thinking}
              choices={THEME_THINKING_CHOICES}
              onChange={(next) => setThinking(next as ThemeThinking)}
              testId="theme-thinking"
              describedBy={`${titleId}-thinking-hint`}
            />

            <p class="theme-transcript-note">
              The transcript keeps anything you must act on visible whatever you choose here: a
              running call, a pending approval, an error and a notice are never folded.
            </p>
          </section>

          <section class="theme-section" aria-labelledby={`${titleId}-advanced`}>
            <div class="theme-section-title">
              <h3 id={`${titleId}-advanced`}>Advanced customization</h3>
            </div>
            <p class="theme-description">
              Full control: add your own selectors, animations, effects, or overrides. No build tools
              needed.
            </p>
            <details class="theme-advanced">
              <summary>
                Custom CSS
                <ChevronDownIcon size={14} />
              </summary>
              <div class="theme-advanced-content">
                <label class="sr-only" for={`${titleId}-css`}>
                  Custom CSS
                </label>
                <p class="theme-advanced-help" id={`${titleId}-css-help`}>
                  Applied live after the application stylesheet. Use{' '}
                  <code>:root {'{ --theme-accent: #eaa06d !important; }'}</code> to adjust inline theme
                  tokens, or target any class.
                </p>
                {/* Trust boundary, said where the CSS is typed. Custom CSS is
                    injected verbatim into this document (no sanitiser beyond the
                    size cap), and an imported theme file carries its own CSS with
                    it, so the file — not just the colours — is something the user
                    is trusting. Saying "cannot load remote resources" would be a
                    lie: `url()` and `@import` both work. */}
                <p
                  class="theme-trust-note"
                  id={`${titleId}-css-trust-note`}
                  role="note"
                  data-testid="theme-css-trust"
                >
                  <AlertIcon size={13} />
                  <span>
                    Only paste CSS you trust. It runs inside this page with the app’s own
                    permissions: it can load remote fonts, images and beacons, and it can hide
                    parts of the interface — including this drawer. An imported theme carries its
                    CSS with it.
                  </span>
                </p>
                <textarea
                  id={`${titleId}-css`}
                  class="theme-code-editor"
                  spellcheck={false}
                  maxLength={MAX_CUSTOM_CSS}
                  aria-describedby={`${titleId}-css-help ${titleId}-css-trust-note`}
                  placeholder={'/* Add any custom CSS here */\n.project-rail { backdrop-filter: blur(12px); }'}
                  value={theme.customCSS}
                  data-testid="theme-custom-css"
                  onInput={(event) => setCustomCSS(event.currentTarget.value)}
                />
                <div class="theme-advanced-actions">
                  <span class="theme-code-hint">Applied live · saved locally</span>
                  <button type="button" class="theme-mini-action" onClick={() => void copyVariables()}>
                    <CopyIcon size={13} />
                    Copy variables
                  </button>
                </div>
              </div>
            </details>
          </section>
        </div>

        <footer class="theme-studio-foot">
          <div class="theme-studio-foot-top">
            <span class="theme-autosave" role="status" data-testid="theme-autosave">
              <span class="theme-dot" aria-hidden="true" />
              {saveError ? (
                saveError
              ) : (
                <>
                  Autosaved in this browser · <strong data-testid="theme-current-name">{activeLabel}</strong>
                </>
              )}
            </span>
            <span
              class={`theme-contrast${contrast < 4.5 ? ' warning' : ''}`}
              data-testid="theme-contrast"
            >
              Contrast {contrast.toFixed(1)}:1 {contrast >= 4.5 ? '✓' : '· Low'}
            </span>
          </div>

          {confirmDelete ? (
            <div class="theme-delete-confirm" role="alertdialog" aria-labelledby={deleteLabelId}>
              <p id={deleteLabelId}>
                Delete “{theme.name}”? This removes it from this browser and cannot be undone.
              </p>
              <div class="theme-delete-actions">
                <button
                  ref={confirmRef}
                  type="button"
                  class="theme-btn danger"
                  onClick={confirmDeleteNow}
                  data-testid="theme-delete-confirm"
                >
                  Delete theme
                </button>
                <button type="button" class="theme-btn" onClick={() => setConfirmDelete(false)}>
                  Keep theme
                </button>
              </div>
            </div>
          ) : null}

          <div class="theme-foot-buttons">
            <button
              type="button"
              class="theme-btn"
              onClick={() => fileRef.current?.click()}
              data-testid="theme-import"
            >
              <UploadIcon size={14} />
              Import
            </button>
            <button
              type="button"
              class="theme-btn"
              onClick={onExport}
              data-testid="theme-export"
            >
              <DownloadIcon size={14} />
              Export
            </button>
            <button
              type="button"
              class="theme-btn"
              onClick={requestDelete}
              disabled={!isCustom}
              aria-label="Delete the current custom theme"
              title={isCustom ? 'Delete this custom theme' : 'Built-in themes cannot be deleted'}
              data-testid="theme-delete"
            >
              <TrashIcon size={14} />
            </button>
            <button
              type="button"
              class="theme-btn primary spacer"
              onClick={openNaming}
              data-testid="theme-save-as-new"
            >
              {dirty ? 'Save theme' : 'Save as new'}
            </button>
          </div>

          <input
            ref={fileRef}
            class="theme-studio-import"
            type="file"
            accept="application/json,.json,.theme.json"
            aria-label="Import a theme JSON file"
            data-testid="theme-import-file"
            onChange={(event) => void onImportFile(event)}
          />
        </footer>
      </aside>
    </div>
  );

  // Portal into <body> like `Dialog` does. `.app-root` is a stacking context
  // (`isolation: isolate`), so `z-index: 110` on the layer was confined inside
  // it and never beat a portalled dialog at 100 — Settings → Theme studio →
  // Ctrl+K left two `aria-modal="true"` surfaces alive, the older one buried
  // under the newer dialog's scrim (DESIGN-AUDIT overlays B6). `document` is
  // absent in a server render and the node-environment suites.
  if (typeof document === 'undefined') return layer;
  return createPortal(layer, document.body);
}