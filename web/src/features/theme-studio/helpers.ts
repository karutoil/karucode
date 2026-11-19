/* Copy and small helpers shared by the Theme Studio panels.
 *
 * The section copy is the mock's own wording (`karucode-v3-themes.html`); the
 * engine keeps the canonical data, so these records only name and describe what
 * `src/theme/` already models.
 */

import type { Theme, ThemeDensity } from '../../theme';

export const DENSITY_LABELS: Record<ThemeDensity, string> = {
  compact: 'Compact',
  comfortable: 'Comfortable',
  spacious: 'Spacious',
};

export const MODE_LABELS = { dark: 'Dark', light: 'Light' } as const;

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

export function groupCopy(group: { id: string; title: string; description: string }): {
  title: string;
  description: string;
} {
  return COLOR_GROUP_COPY[group.id] ?? { title: group.title, description: group.description };
}

/** The miniature's palette, written as custom properties on the preset button. */
export function presetStyle(theme: Theme): string {
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

/** `Blob.text()` is missing in jsdom and older Safari; fall back to FileReader. */
export function readFileText(file: File): Promise<string> {
  if (typeof file.text === 'function') return file.text();
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(typeof reader.result === 'string' ? reader.result : '');
    reader.onerror = () => reject(reader.error ?? new Error('Could not read the file.'));
    reader.readAsText(file);
  });
}

/** After a transient form disappears, keep focus inside the dialog. */
export function refocusSaveButton(panel: HTMLElement | null): void {
  window.setTimeout(() => {
    panel?.querySelector<HTMLButtonElement>('[data-testid="theme-save-as-new"]')?.focus();
  }, 0);
}
