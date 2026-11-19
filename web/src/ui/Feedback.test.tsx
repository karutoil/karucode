import { fireEvent, render, screen } from '@testing-library/preact';
import { describe, expect, it, vi } from 'vitest';
import { Badge, Callout, Chip, Keycap, Skeleton, Spinner, StatusDot } from './Feedback';
import type { FeedbackTone, StatusDotTone } from './types';

describe('Badge', () => {
  it('renders with neutral tone, mono font, and expected tokens by default', () => {
    render(<Badge testId="badge">v1.2.3</Badge>);
    const el = screen.getByTestId('badge');
    expect(el.textContent).toBe('v1.2.3');
    expect(el.className).toContain('text-micro');
    expect(el.className).toContain('rounded-sm');
    expect(el.className).toContain('border');
    expect(el.className).toContain('font-mono');
    expect(el.className).toContain('bg-rail');
    expect(el.className).toContain('border-line');
    expect(el.className).toContain('text-muted');
  });

  it('switches to font-ui when mono is false and forwards class/id', () => {
    render(
      <Badge id="b1" testId="badge" mono={false} class="ml-2">
        Draft
      </Badge>,
    );
    const el = screen.getByTestId('badge');
    expect(el.id).toBe('b1');
    expect(el.className).toContain('font-ui');
    expect(el.className).not.toContain('font-mono');
    expect(el.className).toContain('ml-2');
  });

  it('applies semantic tone classes for every FeedbackTone', () => {
    const expected: Record<FeedbackTone, string[]> = {
      neutral: ['bg-rail', 'border-line', 'text-muted'],
      accent: ['bg-accent-bg', 'border-accent-line', 'text-accent-text'],
      info: ['bg-info/10', 'border-info/35', 'text-info'],
      success: ['bg-success-bg', 'border-success/35', 'text-success'],
      warn: ['bg-warn/10', 'border-warn/40', 'text-warn'],
      danger: ['bg-danger-bg', 'border-danger/35', 'text-danger'],
    };

    for (const [tone, tokens] of Object.entries(expected) as Array<[FeedbackTone, string[]]>) {
      const { unmount } = render(
        <Badge tone={tone} testId={`badge-${tone}`}>
          {tone}
        </Badge>,
      );
      const el = screen.getByTestId(`badge-${tone}`);
      for (const token of tokens) {
        expect(el.className).toContain(token);
      }
      unmount();
    }
  });
});

describe('Chip', () => {
  it('renders label and optional icon without remove button when onRemove is omitted', () => {
    render(
      <Chip testId="chip" tone="accent" icon={<span data-testid="chip-icon">*</span>}>
        Active
      </Chip>,
    );
    const chip = screen.getByTestId('chip');
    expect(chip.textContent).toContain('Active');
    expect(chip.className).toContain('bg-accent-bg');
    expect(chip.className).toContain('text-accent-text');
    expect(screen.getByTestId('chip-icon')).not.toBeNull();
    expect(chip.querySelector('button')).toBeNull();
  });

  it('renders a dismiss button with default or custom aria-label and fires onRemove', () => {
    const onRemove = vi.fn();
    const { rerender } = render(
      <Chip testId="chip" tone="warn" onRemove={onRemove}>
        tag-a
      </Chip>,
    );
    const btn = screen.getByRole('button', { name: 'Remove' });
    expect(btn.getAttribute('type')).toBe('button');
    fireEvent.click(btn);
    expect(onRemove).toHaveBeenCalledTimes(1);

    rerender(
      <Chip testId="chip" tone="danger" onRemove={onRemove} removeLabel="Remove tag-a">
        tag-a
      </Chip>,
    );
    const customBtn = screen.getByRole('button', { name: 'Remove tag-a' });
    fireEvent.click(customBtn);
    expect(onRemove).toHaveBeenCalledTimes(2);
  });
});

describe('StatusDot', () => {
  it('keeps status-dot and status-<tone> classes and is aria-hidden when decorative', () => {
    const tones: StatusDotTone[] = ['running', 'ok', 'danger', 'warn', 'idle', 'streaming'];
    for (const tone of tones) {
      const { unmount } = render(<StatusDot tone={tone} testId={`dot-${tone}`} class="extra" />);
      const dot = screen.getByTestId(`dot-${tone}`);
      expect(dot.className).toBe(`status-dot status-${tone} extra`);
      expect(dot.getAttribute('aria-hidden')).toBe('true');
      expect(dot.getAttribute('aria-label')).toBeNull();
      expect(dot.getAttribute('role')).toBeNull();
      unmount();
    }
  });

  it('exposes aria-label and role="img" when label is provided', () => {
    render(<StatusDot tone="running" label="Session running" testId="dot" />);
    const dot = screen.getByTestId('dot');
    expect(dot.getAttribute('aria-hidden')).toBeNull();
    expect(dot.getAttribute('aria-label')).toBe('Session running');
    expect(dot.getAttribute('role')).toBe('img');
    expect(screen.getByLabelText('Session running')).toBe(dot);
  });
});

describe('Keycap', () => {
  it('renders a <kbd> element with keycap styling tokens', () => {
    render(
      <Keycap id="k1" testId="keycap" class="ml-1">
        ⌘K
      </Keycap>,
    );
    const el = screen.getByTestId('keycap');
    expect(el.tagName).toBe('KBD');
    expect(el.id).toBe('k1');
    expect(el.textContent).toBe('⌘K');
    for (const token of [
      'border-line-strong',
      'rounded-xs',
      'font-mono',
      'text-micro',
      'text-muted',
      'px-2',
      'py-1',
      'ml-1',
    ]) {
      expect(el.className).toContain(token);
    }
  });
});

describe('Callout', () => {
  it('defaults to info tone with role="status" and renders title, icon, body, and actions', () => {
    render(
      <Callout
        testId="callout"
        title="Heads up"
        icon={<span data-testid="callout-icon">i</span>}
        actions={<button type="button">Dismiss</button>}
      >
        Details here
      </Callout>,
    );
    const el = screen.getByTestId('callout');
    expect(el.getAttribute('role')).toBe('status');
    expect(el.className).toContain('bg-info/10');
    expect(el.className).toContain('text-info');
    expect(screen.getByText('Heads up')).not.toBeNull();
    expect(screen.getByText('Details here')).not.toBeNull();
    expect(screen.getByTestId('callout-icon')).not.toBeNull();
    expect(screen.getByRole('button', { name: 'Dismiss' })).not.toBeNull();
  });

  it('defaults to role="alert" for danger tone and allows explicit role override', () => {
    const { rerender } = render(
      <Callout testId="callout" tone="danger">
        Build failed
      </Callout>,
    );
    const danger = screen.getByTestId('callout');
    expect(danger.getAttribute('role')).toBe('alert');
    expect(danger.className).toContain('bg-danger-bg');
    expect(danger.className).toContain('text-danger');

    rerender(
      <Callout testId="callout" tone="danger" role="status">
        Build failed quietly
      </Callout>,
    );
    expect(screen.getByTestId('callout').getAttribute('role')).toBe('status');

    rerender(
      <Callout testId="callout" tone="warn" role="alert">
        Warning!
      </Callout>,
    );
    const warn = screen.getByTestId('callout');
    expect(warn.getAttribute('role')).toBe('alert');
    expect(warn.className).toContain('bg-warn/10');
    expect(warn.className).toContain('text-warn');

    rerender(
      <Callout testId="callout" tone="success">
        Saved
      </Callout>,
    );
    const success = screen.getByTestId('callout');
    expect(success.getAttribute('role')).toBe('status');
    expect(success.className).toContain('bg-success-bg');
    expect(success.className).toContain('text-success');
  });
});

describe('Spinner', () => {
  it('renders decorative SVG spinner with default size 16 and animate-spin', () => {
    render(<Spinner testId="spinner" class="text-accent-text" />);
    const svg = screen.getByTestId('spinner');
    expect(svg.tagName.toLowerCase()).toBe('svg');
    expect(svg.getAttribute('width')).toBe('16');
    expect(svg.getAttribute('height')).toBe('16');
    expect(svg.getAttribute('class')).toContain('animate-spin');
    expect(svg.getAttribute('class')).toContain('text-accent-text');
    expect(svg.getAttribute('aria-hidden')).toBe('true');
    expect(svg.getAttribute('role')).toBeNull();
  });

  it('supports custom size and accessible label', () => {
    render(<Spinner testId="spinner" size={24} label="Loading sessions" />);
    const svg = screen.getByTestId('spinner');
    expect(svg.getAttribute('width')).toBe('24');
    expect(svg.getAttribute('height')).toBe('24');
    expect(svg.getAttribute('aria-hidden')).toBeNull();
    expect(svg.getAttribute('role')).toBe('status');
    expect(svg.getAttribute('aria-label')).toBe('Loading sessions');
    expect(screen.getByRole('status', { name: 'Loading sessions' })).toBe(svg);
  });
});

describe('Skeleton', () => {
  it('renders pulsing placeholder and is aria-hidden by default', () => {
    render(<Skeleton testId="skeleton" class="h-[20px] w-[120px]" />);
    const el = screen.getByTestId('skeleton');
    for (const token of [
      'bg-raised',
      'border',
      'border-line-soft',
      'rounded-sm',
      'animate-pulse',
      'h-[20px]',
      'w-[120px]',
    ]) {
      expect(el.className).toContain(token);
    }
    expect(el.getAttribute('aria-hidden')).toBe('true');
    expect(el.getAttribute('role')).toBeNull();
  });

  it('exposes status role and busy state when label is provided', () => {
    render(<Skeleton testId="skeleton" label="Loading transcript" />);
    const el = screen.getByTestId('skeleton');
    expect(el.getAttribute('aria-hidden')).toBeNull();
    expect(el.getAttribute('role')).toBe('status');
    expect(el.getAttribute('aria-label')).toBe('Loading transcript');
    expect(el.getAttribute('aria-busy')).toBe('true');
  });
});
