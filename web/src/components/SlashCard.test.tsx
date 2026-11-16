import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/preact';
import type { SlashItem } from '../state/chat';
import { SlashCard } from './SlashCard';

function item(overrides: Partial<SlashItem> = {}): SlashItem {
  return {
    kind: 'slash',
    id: 'msg:1',
    at: 0,
    command: 'context',
    content: 'The window is **128k** tokens.',
    ok: true,
    messageId: 1,
    ...overrides,
  };
}

describe('SlashCard', () => {
  it('shows the command it came from and renders its markdown', () => {
    const { container } = render(<SlashCard item={item()} />);
    expect(screen.getByText('/context')).toBeTruthy();
    expect(container.querySelector('strong')?.textContent).toBe('128k');
    expect(container.querySelector('.slash-card.is-error')).toBeNull();
  });

  it('marks a refusal instead of looking like an answer', () => {
    const { container } = render(
      <SlashCard item={item({ ok: false, command: 'compact', content: 'A run is in flight.' })} />,
    );
    expect(container.querySelector('.slash-card.is-error')).toBeTruthy();
    expect(screen.getByText('refused')).toBeTruthy();
    expect(screen.getByLabelText('/compact (refused)')).toBeTruthy();
  });

  it('draws nothing in the body for a command that answered with no text', () => {
    const { container } = render(<SlashCard item={item({ content: '   ' })} />);
    expect(container.querySelector('.slash-card-body')).toBeNull();
    expect(screen.getByText('/context')).toBeTruthy();
  });
});