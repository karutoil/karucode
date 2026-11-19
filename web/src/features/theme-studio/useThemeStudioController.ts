/* The Theme Studio's own behaviour, kept out of its markup.
 *
 * The drawer has three transient states — the inline "save as new" form, the
 * inline delete confirmation, and the half-typed hex fields — plus the two file
 * operations and the "copy variables" export. They all touch the drawer's panel
 * (focus returns to the footer's primary action when a transient form goes
 * away) and they all talk to the theme engine in `src/theme/`, so they live in
 * one hook instead of being spread across six presentational panels.
 *
 * Nothing here uses `window.prompt`/`window.confirm`: naming is an inline form
 * and deleting is an inline confirmation, so the flows stay keyboard- and
 * screen-reader-friendly (and testable).
 */

import type { RefObject, TargetedEvent } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { copyText } from '../../lib/clipboard';
import * as store from '../../state/store';
import {
  MAX_IMPORT_BYTES,
  ROLE_PROPERTIES,
  THEME_TOKEN_ENTRIES,
  ThemeFileError,
  activeTheme,
  buildTokenMap,
  customThemes,
  deleteCustomTheme,
  draftEdited,
  exportThemeJSON,
  importTheme,
  normalizeHex,
  saveThemeAsNew,
  themeFileSlug,
  updateColors,
  type ThemeColorKey,
  type ThemeColorsPatch,
} from '../../theme';
import { readFileText, refocusSaveButton } from './helpers';

export interface ThemeStudioController {
  /** The inline "save as new" form. */
  naming: boolean;
  nameValue: string;
  nameInput: RefObject<HTMLInputElement | null>;
  setNameValue: (value: string) => void;
  openNaming: () => void;
  cancelNaming: () => void;
  /** Close the form silently, for a click that then moves focus elsewhere. */
  dismissNaming: () => void;
  submitName: (event: Event) => void;
  /** The inline delete confirmation. */
  confirmDelete: boolean;
  confirmButton: RefObject<HTMLButtonElement | null>;
  requestDelete: () => void;
  cancelDelete: () => void;
  confirmDeleteNow: () => void;
  /** Import / export / copy-variables. */
  fileInput: RefObject<HTMLInputElement | null>;
  pickImportFile: () => void;
  onImportFile: (event: TargetedEvent<HTMLInputElement, Event>) => Promise<void>;
  exportTheme: () => void;
  copyVariables: () => Promise<void>;
  /** Hex fields while they hold a value the engine has not accepted yet. */
  hexEdits: Record<string, string>;
  onHexInput: (key: ThemeColorKey, raw: string) => void;
  onHexBlur: (key: ThemeColorKey) => void;
}

/**
 * @param open whether the drawer is on screen; the transient forms are reset
 *   every time it reopens, so a half-typed name cannot leak into the next visit.
 * @param panel the dialog panel, used to return focus to the footer's primary
 *   action when the naming form or the delete confirmation goes away.
 */
export function useThemeStudioController({
  open,
  panel,
}: {
  open: boolean;
  panel: RefObject<HTMLElement | null>;
}): ThemeStudioController {
  const [naming, setNaming] = useState(false);
  const [nameValue, setNameValue] = useState('');
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [hexEdits, setHexEdits] = useState<Record<string, string>>({});

  const nameRef = useRef<HTMLInputElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);

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

  const theme = activeTheme.value;

  const openNaming = (): void => {
    setNaming(true);
    setConfirmDelete(false);
    setNameValue(draftEdited.value ? 'My custom theme' : `${theme.name} copy`);
  };

  const cancelNaming = (): void => {
    setNaming(false);
    setNameValue('');
    refocusSaveButton(panel.current);
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
    refocusSaveButton(panel.current);
    store.pushToast(`Saved “${saved.name}” to your themes.`, 'success');
  };

  const isCustom = customThemes.value.some((entry) => entry.id === theme.id);

  const requestDelete = (): void => {
    if (!isCustom) return;
    setNaming(false);
    setConfirmDelete(true);
  };

  const confirmDeleteNow = (): void => {
    const deletedName = activeTheme.value.name;
    if (deleteCustomTheme(activeTheme.value.id)) {
      store.pushToast(`Deleted “${deletedName}”.`, 'success');
    }
    setConfirmDelete(false);
    refocusSaveButton(panel.current);
  };

  const pickImportFile = (): void => {
    fileRef.current?.click();
  };

  const onImportFile = async (event: TargetedEvent<HTMLInputElement, Event>): Promise<void> => {
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

  const exportTheme = (): void => {
    try {
      const current = activeTheme.value;
      if (typeof URL.createObjectURL !== 'function') throw new Error('No object URLs');
      const blob = new Blob([exportThemeJSON(current)], { type: 'application/json' });
      const href = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = href;
      anchor.download = `karucode-${themeFileSlug(current.name)}.theme.json`;
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
    const current = activeTheme.value;
    const tokens = buildTokenMap(current);
    const variables = [
      ...THEME_TOKEN_ENTRIES.map(([, property]) => property),
      ...ROLE_PROPERTIES,
    ]
      .filter((property) => tokens[property] !== undefined)
      // applyTheme writes these properties inline on <html>; a normal :root
      // declaration loses that cascade contest even though the editor is later.
      .map((property) => `  ${property}: ${tokens[property]} !important;`)
      .join('\n');
    const code = `/* !important lets Custom CSS replace the engine's inline token values. */\n/* ${current.name} — ${current.mode} */\n:root {\n${variables}\n}\n${current.customCSS}`;
    // Through the shared helper, which has the legacy `execCommand` fallback:
    // this is the only path on a plain-HTTP LAN origin, where the Clipboard API
    // does not exist at all.
    const copied = await copyText(code);
    store.pushToast(
      copied ? 'Theme variables copied.' : 'Clipboard is unavailable in this browser.',
      copied ? 'success' : 'error',
    );
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

  return {
    naming,
    nameValue,
    nameInput: nameRef,
    setNameValue,
    openNaming,
    cancelNaming,
    dismissNaming: () => setNaming(false),
    submitName,
    confirmDelete,
    confirmButton: confirmRef,
    requestDelete,
    cancelDelete: () => setConfirmDelete(false),
    confirmDeleteNow,
    fileInput: fileRef,
    pickImportFile,
    onImportFile,
    exportTheme,
    copyVariables,
    hexEdits,
    onHexInput,
    onHexBlur,
  };
}
