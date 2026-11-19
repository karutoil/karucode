import { afterEach, describe, expect, it } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { UiGallery } from './UiGallery';
import type {
  ButtonVariant,
  CalloutTone,
  FeedbackTone,
  StatusDotTone,
  TextSize,
  TextTone,
} from './index';

afterEach(() => {
  document.body.innerHTML = '';
  delete document.documentElement.dataset.theme;
  delete document.documentElement.dataset.mode;
  delete document.documentElement.dataset.density;
});

describe('UiGallery', () => {
  it('renders the gallery root and all four primitive sections', () => {
    render(<UiGallery />);

    expect(screen.getByTestId('ui-gallery-root')).not.toBeNull();
    expect(screen.getByTestId('gallery-section-actions')).not.toBeNull();
    expect(screen.getByTestId('gallery-section-forms')).not.toBeNull();
    expect(screen.getByTestId('gallery-section-typography')).not.toBeNull();
    expect(screen.getByTestId('gallery-section-feedback')).not.toBeNull();

    const buttonVariants: readonly ButtonVariant[] = [
      'secondary',
      'primary',
      'ghost',
      'danger',
      'danger-ghost',
      'stop',
    ];
    for (const variant of buttonVariants) {
      expect(screen.getByTestId(`gallery-btn-md-${variant}`)).not.toBeNull();
      expect(screen.getByTestId(`gallery-btn-sm-${variant}`)).not.toBeNull();
    }
    expect((screen.getByTestId('gallery-btn-loading') as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId('gallery-btn-disabled') as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId('gallery-icon-btn-md')).not.toBeNull();
    expect(screen.getByTestId('gallery-link-accent')).not.toBeNull();

    const textSizes: readonly TextSize[] = [
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
    for (const size of textSizes) {
      expect(screen.getByTestId(`gallery-text-size-${size}`)).not.toBeNull();
    }

    const textTones: readonly TextTone[] = [
      'default',
      'secondary',
      'muted',
      'accent',
      'success',
      'warn',
      'danger',
      'info',
    ];
    for (const tone of textTones) {
      expect(screen.getByTestId(`gallery-text-tone-${tone}`)).not.toBeNull();
    }

    for (const surface of ['surface', 'raised', 'inset', 'sunken']) {
      expect(screen.getByTestId(`gallery-card-${surface}`)).not.toBeNull();
    }
    expect(screen.getByTestId('gallery-setting-row-toggle')).not.toBeNull();
    expect(screen.getByTestId('gallery-setting-row-select')).not.toBeNull();

    const badgeTones: readonly FeedbackTone[] = [
      'neutral',
      'accent',
      'info',
      'success',
      'warn',
      'danger',
    ];
    for (const tone of badgeTones) {
      expect(screen.getByTestId(`gallery-badge-${tone}`)).not.toBeNull();
    }

    const dotTones: readonly StatusDotTone[] = [
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
    for (const tone of dotTones) {
      expect(screen.getByTestId(`gallery-dot-${tone}`)).not.toBeNull();
    }

    const calloutTones: readonly CalloutTone[] = ['info', 'warn', 'danger', 'success'];
    for (const tone of calloutTones) {
      expect(screen.getByTestId(`gallery-callout-${tone}`)).not.toBeNull();
    }

    expect(screen.getByTestId('gallery-spinner-sm')).not.toBeNull();
    expect(screen.getByTestId('gallery-skeleton-1')).not.toBeNull();
    expect(screen.getByTestId('gallery-empty-state-card')).not.toBeNull();
    expect(screen.getByRole('heading', { name: 'What are we building?' })).not.toBeNull();
  });

  it('switches theme, color mode, and density', () => {
    render(<UiGallery />);

    expect(document.documentElement.dataset.theme).toBe('ember');
    expect(document.documentElement.dataset.mode).toBe('dark');
    expect(document.documentElement.dataset.density).toBe('comfortable');

    const themeSelect = screen.getByTestId('gallery-theme-select') as HTMLSelectElement;
    fireEvent.change(themeSelect, { target: { value: 'daybreak' } });
    expect(document.documentElement.dataset.theme).toBe('daybreak');
    expect(document.documentElement.dataset.mode).toBe('light');

    fireEvent.click(screen.getByTestId('gallery-mode-dark'));
    expect(document.documentElement.dataset.mode).toBe('dark');
    expect(screen.getByTestId('gallery-mode-dark').getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByTestId('gallery-mode-light').getAttribute('aria-pressed')).toBe('false');

    fireEvent.click(screen.getByTestId('gallery-mode-light'));
    expect(document.documentElement.dataset.mode).toBe('light');
    expect(screen.getByTestId('gallery-mode-light').getAttribute('aria-pressed')).toBe('true');

    const densitySelect = screen.getByTestId('gallery-density-select') as HTMLSelectElement;
    fireEvent.change(densitySelect, { target: { value: 'compact' } });
    expect(document.documentElement.dataset.density).toBe('compact');

    fireEvent.change(densitySelect, { target: { value: 'spacious' } });
    expect(document.documentElement.dataset.density).toBe('spacious');
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
});
