import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import {
  BUILTIN_THEMES,
  DENSITY_SCALES,
  THEME_DENSITIES,
  resolvePalette,
  type ThemeDensity,
} from '../theme';
import { UiGallery } from './UiGallery';
import type {
  ButtonVariant,
  CalloutTone,
  FeedbackTone,
  OverlayRole,
  StatusDotTone,
  SurfaceTone,
  TextSize,
  TextTone,
} from './index';

const BUTTON_VARIANTS: readonly ButtonVariant[] = [
  'secondary',
  'primary',
  'ghost',
  'danger',
  'danger-ghost',
  'stop',
];

const TEXT_SIZES: readonly TextSize[] = [
  'micro',
  'tiny',
  'small',
  'label',
  'message',
  'body',
  'lead',
  'title',
  'heading',
  'display',
  'hero',
];

const TEXT_TONES: readonly TextTone[] = [
  'default',
  'secondary',
  'muted',
  'accent',
  'success',
  'warn',
  'danger',
  'info',
];

const CARD_SURFACES: readonly SurfaceTone[] = ['bg', 'surface', 'raised', 'inset', 'sunken', 'rail'];

const FEEDBACK_TONES: readonly FeedbackTone[] = [
  'neutral',
  'accent',
  'info',
  'success',
  'warn',
  'danger',
];

const STATUS_DOT_TONES: readonly StatusDotTone[] = [
  'running',
  'ok',
  'error',
  'danger',
  'warn',
  'approval',
  'interrupted',
  'cancelled',
  'stopped',
  'muted',
  'idle',
  'tool',
  'streaming',
  'thinking',
];

const CALLOUT_TONES: readonly CalloutTone[] = ['info', 'warn', 'danger', 'success'];

const OVERLAY_ROLES: readonly OverlayRole[] = [
  'compact',
  'standard',
  'wide',
  'tall',
  'drawer',
  'full',
  'palette',
];

const htmlToken = (property: string): string =>
  document.documentElement.style.getPropertyValue(property);

afterEach(() => {
  vi.restoreAllMocks();
  document.body.innerHTML = '';
  delete document.documentElement.dataset.theme;
  delete document.documentElement.dataset.mode;
  delete document.documentElement.dataset.density;
});

describe('UiGallery', () => {
  it('renders all five primitive sections with their headings', () => {
    render(<UiGallery />);

    expect(screen.getByTestId('ui-gallery-root')).not.toBeNull();
    expect(screen.getByTestId('gallery-section-actions')).not.toBeNull();
    expect(screen.getByTestId('gallery-section-forms')).not.toBeNull();
    expect(screen.getByTestId('gallery-section-typography')).not.toBeNull();
    expect(screen.getByTestId('gallery-section-feedback')).not.toBeNull();
    expect(screen.getByTestId('gallery-section-overlays')).not.toBeNull();

    for (const name of [
      'Buttons, links & confirmation',
      'Fields, inputs & toggles',
      'Type scale, tones, cards & setting rows',
      'Badges, chips, status dots, callouts, tabs & empty state',
      'Overlay: one shell, seven measures',
    ]) {
      expect(screen.getByRole('heading', { name }), name).not.toBeNull();
    }
  });

  it('shows every Button variant in md and sm with loading, disabled and icons', () => {
    render(<UiGallery />);

    for (const variant of BUTTON_VARIANTS) {
      const md = screen.getByTestId(`gallery-btn-md-${variant}`);
      const sm = screen.getByTestId(`gallery-btn-sm-${variant}`);
      expect(md.getAttribute('data-variant'), variant).toBe(variant);
      expect(sm.getAttribute('data-variant'), variant).toBe(variant);
      expect(md.getAttribute('data-size'), variant).toBe('md');
      expect(sm.getAttribute('data-size'), variant).toBe('sm');
      expect(md.getAttribute('class'), variant).toContain('h-[30px]');
      expect(sm.getAttribute('class'), variant).toContain('h-[26px]');
    }

    const leading = screen.getByTestId('gallery-btn-icon-left');
    const trailing = screen.getByTestId('gallery-btn-icon-right');
    expect(leading.querySelector('svg')).not.toBeNull();
    expect(trailing.querySelector('svg')).not.toBeNull();
    // The icon slots are slots: the left icon precedes the label, the right one follows it.
    expect(leading.firstElementChild?.tagName.toLowerCase()).toBe('svg');
    expect(trailing.lastElementChild?.tagName.toLowerCase()).toBe('svg');

    const loading = screen.getByTestId('gallery-btn-loading') as HTMLButtonElement;
    expect(loading.disabled).toBe(true);
    expect(loading.getAttribute('aria-busy')).toBe('true');
    expect(loading.querySelector('svg')).not.toBeNull();

    const disabled = screen.getByTestId('gallery-btn-disabled') as HTMLButtonElement;
    expect(disabled.disabled).toBe(true);
    expect(disabled.getAttribute('aria-busy')).toBeNull();
  });

  it('shows IconButton and LinkButton in every state the gallery claims', () => {
    render(<UiGallery />);

    expect((screen.getByTestId('gallery-icon-btn-md') as HTMLButtonElement).disabled).toBe(false);
    expect(
      screen.getByTestId('gallery-icon-btn-md').querySelector('svg'),
      'icon button renders its icon',
    ).not.toBeNull();
    expect(screen.getByTestId('gallery-icon-btn-sm').getAttribute('data-size')).toBe('sm');
    expect(screen.getByTestId('gallery-icon-btn-pressed').getAttribute('aria-pressed')).toBe('true');
    expect((screen.getByTestId('gallery-icon-btn-disabled') as HTMLButtonElement).disabled).toBe(true);

    expect(screen.getByTestId('gallery-link-muted').getAttribute('data-tone')).toBe('muted');
    expect(screen.getByTestId('gallery-link-accent').getAttribute('data-tone')).toBe('accent');
    expect((screen.getByTestId('gallery-link-disabled') as HTMLButtonElement).disabled).toBe(true);
  });

  it('shows every form primitive: Field hint/error/row, Input, Textarea, Select, Checkbox, Toggle', () => {
    render(<UiGallery />);

    // Field: a label, a hint and an error message each reach the DOM.
    const normalField = screen.getByTestId('gallery-field-normal');
    expect(normalField.textContent).toContain('Workspace name');
    expect(normalField.textContent).toContain('Displayed in the project rail.');

    const invalidField = screen.getByTestId('gallery-field-invalid');
    expect(invalidField.getAttribute('data-invalid')).toBe('true');
    expect(invalidField.querySelector('[role="alert"]')?.textContent).toBe('Token format is invalid.');
    expect(screen.getByTestId('gallery-input-invalid').getAttribute('aria-invalid')).toBe('true');

    // Field row layout, and the small numeric Input inside it.
    const rowField = screen.getByTestId('gallery-field-row');
    expect(rowField.getAttribute('class')).toContain('grid');
    const rowInput = screen.getByTestId('gallery-input-row') as HTMLInputElement;
    expect(rowInput.type).toBe('number');
    expect(rowInput.max).toBe('16384');

    expect((screen.getByTestId('gallery-input-mono') as HTMLInputElement).getAttribute('class')).toContain('font-mono');
    expect((screen.getByTestId('gallery-input-disabled') as HTMLInputElement).disabled).toBe(true);
    expect(screen.getByTestId('gallery-textarea-normal').tagName).toBe('TEXTAREA');
    expect(screen.getByTestId('gallery-textarea-invalid').getAttribute('aria-invalid')).toBe('true');
    expect(screen.getByTestId('gallery-select-normal').tagName).toBe('SELECT');
    expect((screen.getByTestId('gallery-select-disabled') as HTMLSelectElement).disabled).toBe(true);

    expect((screen.getByTestId('gallery-checkbox') as HTMLInputElement).checked).toBe(true);
    expect((screen.getByTestId('gallery-checkbox-disabled') as HTMLInputElement).disabled).toBe(true);
    expect(screen.getByTestId('gallery-toggle').getAttribute('role')).toBe('switch');
    expect((screen.getByTestId('gallery-toggle-disabled') as HTMLButtonElement).disabled).toBe(true);
  });

  it('shows Stack/Inline, all Card surfaces, every Text size and tone, SectionHeader and SettingRow', () => {
    render(<UiGallery />);

    expect(screen.getByTestId('gallery-stack-demo')).not.toBeNull();
    const inlineBetween = screen.getByTestId('gallery-inline-between');
    expect(inlineBetween.getAttribute('class')).toContain('justify-between');
    expect(inlineBetween.getAttribute('class')).toContain('items-center');

    for (const surface of CARD_SURFACES) {
      expect(screen.getByTestId(`gallery-card-${surface}`).getAttribute('class'), surface).toContain(
        `bg-${surface}`,
      );
    }

    for (const size of TEXT_SIZES) {
      expect(screen.getByTestId(`gallery-text-size-${size}`).getAttribute('class'), size).toContain(
        `text-${size}`,
      );
    }
    for (const tone of TEXT_TONES) {
      const expected =
        tone === 'default' ? 'text-text' : tone === 'secondary' ? 'text-text-secondary' : `text-${tone}`;
      expect(screen.getByTestId(`gallery-text-tone-${tone}`).getAttribute('class'), tone).toContain(
        expected,
      );
    }

    expect(screen.getByTestId('gallery-setting-row-toggle').textContent).toContain(
      'Stream markdown incrementally',
    );
    const selectRow = screen.getByTestId('gallery-setting-row-select');
    expect(selectRow.querySelector('label')?.getAttribute('for')).toBe('gallery-setting-select');
    expect(screen.getByTestId('gallery-setting-select')).not.toBeNull();
  });

  it('shows every feedback primitive: Badge, Chip, StatusDot, Keycap, Callout, Spinner, Skeleton', () => {
    render(<UiGallery />);

    for (const tone of FEEDBACK_TONES) {
      const badge = screen.getByTestId(`gallery-badge-${tone}`);
      // `neutral` is the rail/muted rung and `accent` paints the accent *ink*
      // (`text-accent-text`), which is the token twin of its fill.
      const expected = tone === 'neutral' ? 'text-muted' : tone === 'accent' ? 'text-accent-text' : `text-${tone}`;
      expect(badge.getAttribute('class'), tone).toContain(expected);
    }
    expect(screen.getByTestId('gallery-badge-sans').getAttribute('class')).toContain('font-ui');

    expect(screen.getByTestId('gallery-chip-rust')).not.toBeNull();
    expect(screen.getByTestId('gallery-chip-breaking')).not.toBeNull();

    for (const tone of STATUS_DOT_TONES) {
      const dot = screen.getByTestId(`gallery-dot-${tone}`);
      expect(dot.getAttribute('class'), tone).toContain(`status-${tone}`);
      expect(dot.getAttribute('aria-label'), tone).toBe(`Status ${tone}`);
    }

    expect(screen.getByTestId('gallery-keycap-ctrl').tagName).toBe('KBD');
    expect(screen.getByTestId('gallery-keycap-shift').textContent).toBe('Shift');
    expect(screen.getByTestId('gallery-keycap-l').textContent).toBe('L');

    for (const tone of CALLOUT_TONES) {
      const callout = screen.getByTestId(`gallery-callout-${tone}`);
      // `danger` announces itself, everything else is a status; the shell owns that decision.
      expect(callout.getAttribute('role'), tone).toBe(tone === 'danger' ? 'alert' : 'status');
      expect(callout.textContent, tone).toContain(`${tone.toUpperCase()} notice`);
    }

    expect(screen.getByTestId('gallery-spinner-sm').getAttribute('role')).toBe('status');
    expect(screen.getByTestId('gallery-skeleton-1').getAttribute('aria-busy')).toBe('true');
    expect(screen.getByTestId('gallery-skeleton-2').getAttribute('aria-hidden')).toBe('true');

    expect(screen.getByTestId('gallery-empty-state-card')).not.toBeNull();
    expect(screen.getByRole('heading', { name: 'What are we building?' })).not.toBeNull();
  });

  it('offers an Overlay preview for all seven shell roles', () => {
    render(<UiGallery />);

    expect(screen.getByTestId('gallery-overlay-status').textContent).toContain('none');
    expect(screen.queryByRole('dialog')).toBeNull();

    for (const role of OVERLAY_ROLES) {
      fireEvent.click(screen.getByTestId(`gallery-open-overlay-${role}`));

      const dialog = screen.getByRole('dialog', { name: `Overlay · ${role}` });
      expect(dialog.getAttribute('data-role'), role).toBe(role);
      expect(dialog.getAttribute('class'), role).toContain(`overlay-${role}`);
      expect(dialog.getAttribute('aria-modal'), role).toBe('true');

      fireEvent.click(screen.getByTestId('gallery-overlay-done'));
      expect(screen.queryByRole('dialog'), role).toBeNull();
      expect(screen.getByTestId('gallery-overlay-status').textContent, role).toContain(
        `closed (${role})`,
      );
    }
  });

  it('opens, cancels, and confirms ConfirmDialog', () => {
    render(<UiGallery />);

    expect(screen.queryByRole('dialog')).toBeNull();
    expect(screen.getByTestId('gallery-confirm-status').textContent).toContain('none');

    fireEvent.click(screen.getByTestId('gallery-open-confirm-primary'));
    expect(screen.getByRole('dialog', { name: 'Apply gallery preset?' })).not.toBeNull();

    fireEvent.click(screen.getByTestId('gallery-confirm-dialog-cancel'));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(screen.getByTestId('gallery-confirm-status').textContent).toContain('cancelled (primary)');

    fireEvent.click(screen.getByTestId('gallery-open-confirm-danger'));
    const dangerDialog = screen.getByRole('dialog', { name: 'Reset theme overrides?' });
    expect(dangerDialog).not.toBeNull();

    const confirmBtn = screen.getByTestId('gallery-confirm-dialog-confirm');
    expect(confirmBtn.getAttribute('data-variant')).toBe('danger');
    fireEvent.click(confirmBtn);
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(screen.getByTestId('gallery-confirm-status').textContent).toContain('confirmed (danger)');
  });

  it('switches Tabs and renders corresponding TabPanel content', () => {
    render(<UiGallery />);

    expect(screen.getByTestId('gallery-tabpanel-overview')).not.toBeNull();
    expect(screen.queryByTestId('gallery-tabpanel-tokens')).toBeNull();

    fireEvent.click(screen.getByRole('tab', { name: /Tokens/ }));
    expect(screen.queryByTestId('gallery-tabpanel-overview')).toBeNull();
    expect(screen.getByTestId('gallery-tabpanel-tokens')).not.toBeNull();

    fireEvent.click(screen.getByRole('tab', { name: /Shortcuts/ }));
    expect(screen.queryByTestId('gallery-tabpanel-tokens')).toBeNull();
    expect(screen.getByTestId('gallery-tabpanel-shortcuts')).not.toBeNull();
  });

  it('toggles Checkbox and Toggle and dismisses/restores Chips', () => {
    render(<UiGallery />);

    const checkbox = screen.getByTestId('gallery-checkbox') as HTMLInputElement;
    expect(checkbox.checked).toBe(true);
    fireEvent.click(checkbox);
    expect(checkbox.checked).toBe(false);

    const toggle = screen.getByTestId('gallery-toggle');
    expect(toggle.getAttribute('aria-checked')).toBe('false');
    fireEvent.click(toggle);
    expect(toggle.getAttribute('aria-checked')).toBe('true');

    expect(screen.getByTestId('gallery-chip-rust')).not.toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Remove rust' }));
    expect(screen.queryByTestId('gallery-chip-rust')).toBeNull();

    fireEvent.click(screen.getByTestId('gallery-chips-reset'));
    expect(screen.getByTestId('gallery-chip-rust')).not.toBeNull();
  });

  it('repaints <html> for all eight themes without a console error', () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    render(<UiGallery />);

    expect(BUILTIN_THEMES).toHaveLength(8);
    const themeSelect = screen.getByTestId('gallery-theme-select') as HTMLSelectElement;
    expect(themeSelect.options).toHaveLength(8);

    for (const theme of BUILTIN_THEMES) {
      fireEvent.change(themeSelect, { target: { value: theme.id } });

      const root = document.documentElement;
      expect(root.dataset.theme, theme.id).toBe(theme.id);
      // A built-in theme carries its own mode, so the select moves both.
      expect(root.dataset.mode, theme.id).toBe(theme.mode);
      expect(root.dataset.density, theme.id).toBe('comfortable');
      expect(htmlToken('--theme-bg'), theme.id).toBe(resolvePalette(theme).bg);
      expect(htmlToken('--theme-accent'), theme.id).toBe(resolvePalette(theme).accent);
      expect(screen.getByTestId('gallery-theme-summary').textContent, theme.id).toContain(theme.id);
      expect(themeSelect.value, theme.id).toBe(theme.id);
    }

    expect(errorSpy).not.toHaveBeenCalled();
  });

  it('switches colour mode through the recorded theme, repainting the tokens', () => {
    render(<UiGallery />);

    expect(document.documentElement.dataset.theme).toBe('ember');
    expect(document.documentElement.dataset.mode).toBe('dark');
    expect(htmlToken('--theme-bg')).toBe(resolvePalette(BUILTIN_THEMES[0]!).bg);

    fireEvent.change(screen.getByTestId('gallery-theme-select'), { target: { value: 'daybreak' } });
    expect(document.documentElement.dataset.mode).toBe('light');
    const daybreakBg = htmlToken('--theme-bg');
    expect(daybreakBg).toBe(resolvePalette(BUILTIN_THEMES[5]!).bg);

    fireEvent.click(screen.getByTestId('gallery-mode-dark'));
    expect(document.documentElement.dataset.mode).toBe('dark');
    expect(htmlToken('--theme-bg'), 'dark re-bases the palette').not.toBe(daybreakBg);
    expect(screen.getByTestId('gallery-mode-dark').getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByTestId('gallery-mode-light').getAttribute('aria-pressed')).toBe('false');
    // The theme choice survives a mode switch; only the palette is re-based.
    expect(document.documentElement.dataset.theme).toBe('daybreak');

    fireEvent.click(screen.getByTestId('gallery-mode-light'));
    expect(document.documentElement.dataset.mode).toBe('light');
    expect(screen.getByTestId('gallery-mode-light').getAttribute('aria-pressed')).toBe('true');
    expect(htmlToken('--theme-bg')).toBe(daybreakBg);
  });

  it('switches all three densities and rescales the spacing/type tokens', () => {
    render(<UiGallery />);

    const densitySelect = screen.getByTestId('gallery-density-select') as HTMLSelectElement;
    expect([...densitySelect.options].map((option) => option.value)).toEqual([...THEME_DENSITIES]);

    for (const density of THEME_DENSITIES as readonly ThemeDensity[]) {
      fireEvent.change(densitySelect, { target: { value: density } });

      expect(document.documentElement.dataset.density, density).toBe(density);
      expect(htmlToken('--fs-body'), density).toBe(`${DENSITY_SCALES[density].body}px`);
      expect(htmlToken('--space-1'), density).not.toBe('');
      expect(screen.getByTestId('gallery-theme-summary').textContent, density).toContain(density);
    }

    // compact and spacious are the two ends of the same ramp: they cannot agree.
    fireEvent.change(densitySelect, { target: { value: 'compact' } });
    const compactBody = htmlToken('--fs-body');
    fireEvent.change(densitySelect, { target: { value: 'spacious' } });
    expect(htmlToken('--fs-body')).not.toBe(compactBody);
  });
});
