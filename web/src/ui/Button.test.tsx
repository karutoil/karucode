import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { Button, IconButton, LinkButton } from './Button';

describe('Button', () => {
  it('defaults to type="button", variant="secondary", size="md" without legacy .button class', () => {
    const onClick = vi.fn();
    render(
      <Button testId="save-btn" onClick={onClick}>
        Save
      </Button>,
    );

    const btn = screen.getByTestId('save-btn') as HTMLButtonElement;
    expect(btn.tagName).toBe('BUTTON');
    expect(btn.getAttribute('type')).toBe('button');
    expect(btn.getAttribute('data-variant')).toBe('secondary');
    expect(btn.getAttribute('data-size')).toBe('md');
    expect(btn.classList.contains('button')).toBe(false);
    expect(btn.hasAttribute('aria-busy')).toBe(false);
    expect(btn.disabled).toBe(false);

    fireEvent.click(btn);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('supports every variant and size token combination', () => {
    const variants = ['secondary', 'primary', 'ghost', 'danger', 'danger-ghost', 'stop'] as const;
    for (const variant of variants) {
      const { unmount } = render(
        <Button variant={variant} size="sm" testId={`btn-${variant}`}>
          {variant}
        </Button>,
      );
      const btn = screen.getByTestId(`btn-${variant}`);
      expect(btn.getAttribute('data-variant')).toBe(variant);
      expect(btn.getAttribute('data-size')).toBe('sm');
      expect(btn.className).toContain('h-[26px]');
      unmount();
    }
  });

  it('supports explicit type, icons, aria attributes, title, and extra class', () => {
    render(
      <Button
        type="submit"
        variant="primary"
        icon={<span data-testid="left-icon">L</span>}
        iconRight={<span data-testid="right-icon">R</span>}
        title="Submit form"
        aria-label="Submit action"
        aria-expanded={true}
        aria-pressed={false}
        class="ml-auto"
      >
        Submit
      </Button>,
    );

    const btn = screen.getByRole('button', { name: 'Submit action' });
    expect(btn.getAttribute('type')).toBe('submit');
    expect(btn.getAttribute('title')).toBe('Submit form');
    expect(btn.getAttribute('aria-expanded')).toBe('true');
    expect(btn.getAttribute('aria-pressed')).toBe('false');
    expect(btn.className).toContain('ml-auto');
    expect(btn.className).toContain('h-[30px]');
    expect(screen.getByTestId('left-icon')).not.toBeNull();
    expect(screen.getByTestId('right-icon')).not.toBeNull();
  });

  it('suppresses clicks when disabled', () => {
    const onClick = vi.fn();
    render(
      <Button disabled onClick={onClick}>
        Disabled
      </Button>,
    );

    const btn = screen.getByRole('button', { name: 'Disabled' }) as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    fireEvent.click(btn);
    expect(onClick).not.toHaveBeenCalled();
  });

  it('disables button, sets aria-busy, and renders spinner when loading', () => {
    const onClick = vi.fn();
    render(
      <Button
        loading
        icon={<svg data-testid="left-icon" aria-hidden="true" />}
        iconRight={<svg data-testid="right-icon" aria-hidden="true" />}
        onClick={onClick}
      >
        Saving
      </Button>,
    );

    const btn = screen.getByRole('button', { name: 'Saving' }) as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    expect(btn.getAttribute('aria-busy')).toBe('true');
    expect(screen.queryByTestId('left-icon')).toBeNull();
    expect(screen.getByTestId('right-icon')).not.toBeNull();

    const spinner = btn.querySelector('svg');
    expect(spinner).not.toBeNull();
    expect(spinner?.getAttribute('aria-hidden')).toBe('true');
    expect(spinner?.getAttribute('class')).toContain('animate-spin');

    fireEvent.click(btn);
    expect(onClick).not.toHaveBeenCalled();
  });
});

describe('IconButton', () => {
  it('requires label for aria-label, defaults to type="button", and omits legacy .icon-button class', () => {
    const onClick = vi.fn();
    render(
      <IconButton
        label="Toggle sidebar"
        icon={<svg data-testid="glyph" />}
        onClick={onClick}
        title="Sidebar"
        testId="sidebar-toggle"
      />,
    );

    const btn = screen.getByRole('button', { name: 'Toggle sidebar' }) as HTMLButtonElement;
    expect(btn.getAttribute('type')).toBe('button');
    expect(btn.getAttribute('title')).toBe('Sidebar');
    expect(btn.getAttribute('data-testid')).toBe('sidebar-toggle');
    expect(btn.getAttribute('data-size')).toBe('md');
    expect(btn.classList.contains('icon-button')).toBe(false);
    expect(btn.className).toContain('w-[30px]');
    expect(btn.hasAttribute('aria-pressed')).toBe(false);
    expect(btn.hasAttribute('aria-expanded')).toBe(false);
    expect(screen.getByTestId('glyph')).not.toBeNull();

    fireEvent.click(btn);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('wires size="sm", aria-pressed, aria-expanded, and disabled', () => {
    const onClick = vi.fn();
    const { rerender } = render(
      <IconButton
        label="Pin panel"
        icon={<svg />}
        size="sm"
        pressed={true}
        expanded={false}
        onClick={onClick}
      />,
    );

    const btn = screen.getByRole('button', { name: 'Pin panel' }) as HTMLButtonElement;
    expect(btn.getAttribute('data-size')).toBe('sm');
    expect(btn.className).toContain('w-[26px]');
    expect(btn.className).toContain('h-[26px]');
    expect(btn.getAttribute('aria-pressed')).toBe('true');
    expect(btn.getAttribute('aria-expanded')).toBe('false');

    rerender(
      <IconButton
        label="Pin panel"
        icon={<svg />}
        size="sm"
        pressed={false}
        expanded={true}
        disabled
        onClick={onClick}
      />,
    );
    expect(btn.getAttribute('aria-pressed')).toBe('false');
    expect(btn.getAttribute('aria-expanded')).toBe('true');
    expect(btn.disabled).toBe(true);
    fireEvent.click(btn);
    expect(onClick).not.toHaveBeenCalled();
  });
});

describe('LinkButton', () => {
  it('defaults to type="button" and tone="muted" and fires onClick when enabled', () => {
    const onClick = vi.fn();
    render(
      <LinkButton onClick={onClick} testId="reset-link" title="Reset filters">
        Reset
      </LinkButton>,
    );

    const btn = screen.getByRole('button', { name: 'Reset' }) as HTMLButtonElement;
    expect(btn.getAttribute('type')).toBe('button');
    expect(btn.getAttribute('data-tone')).toBe('muted');
    expect(btn.getAttribute('title')).toBe('Reset filters');
    expect(btn.classList.contains('link-button')).toBe(false);

    fireEvent.click(btn);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('supports tone="accent" and disabled state', () => {
    const onClick = vi.fn();
    render(
      <LinkButton tone="accent" disabled onClick={onClick}>
        Learn more
      </LinkButton>,
    );

    const btn = screen.getByRole('button', { name: 'Learn more' }) as HTMLButtonElement;
    expect(btn.getAttribute('data-tone')).toBe('accent');
    expect(btn.disabled).toBe(true);
    fireEvent.click(btn);
    expect(onClick).not.toHaveBeenCalled();
  });
});
