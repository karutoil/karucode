import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/preact';
import { ThinkingBlock } from './ThinkingBlock';

describe('ThinkingBlock', () => {
  it('shows a chip with a way in when the reasoning is hidden', () => {
    render(<ThinkingBlock reasoning="I should read the config first." streaming={false} />);

    const details = document.querySelector('.thinking');
    expect(details?.hasAttribute('open')).toBe(false);
    expect(screen.getByText('Thought')).not.toBeNull();
    // A bare chip gives no hint that there is anything behind it.
    expect(screen.getByText('Show reasoning')).not.toBeNull();
    expect(document.querySelector('.thinking-preview')).toBeNull();
    // The text is in the DOM (the disclosure is CSS/DOM-native), just collapsed.
    expect(document.querySelector('.thinking-body')?.textContent).toContain('read the config');
  });

  it('previews the first line of reasoning in preview mode', () => {
    render(
      <ThinkingBlock
        reasoning={'The failing test is in paths.rs. Then I will check the sibling entry.'}
        streaming={false}
        mode="preview"
      />,
    );

    const preview = document.querySelector('.thinking-preview');
    expect(preview?.textContent).toBe('The failing test is in paths.rs.');
    expect(document.querySelector('.thinking')?.hasAttribute('open')).toBe(false);
    // No redundant hint when the line itself already says what this is.
    expect(screen.queryByText('Show reasoning')).toBeNull();
  });

  it('flattens multi-line reasoning into one readable line', () => {
    render(
      <ThinkingBlock
        reasoning={'\n\n  Reading the tool result.\n\n  Then deciding.  '}
        streaming={false}
        mode="preview"
      />,
    );
    expect(document.querySelector('.thinking-preview')?.textContent).toBe('Reading the tool result.');
  });

  it('renders open in expanded mode', () => {
    render(<ThinkingBlock reasoning="Full thought." streaming={false} mode="expanded" />);
    expect(document.querySelector('.thinking')?.hasAttribute('open')).toBe(true);
    expect(document.querySelector('.thinking-preview')).toBeNull();
  });

  it('opens itself and shows a live dot while the model is thinking', () => {
    render(<ThinkingBlock reasoning="Considering options" streaming />);

    const details = document.querySelector('.thinking');
    expect(details?.hasAttribute('open')).toBe(true);
    expect(screen.getByText('Thinking…')).not.toBeNull();
    expect(document.querySelector('.thinking-live')).not.toBeNull();
  });

  it('renders reasoning as text, never as markup', () => {
    render(
      <ThinkingBlock reasoning={'<img src=x onerror=alert(1)>'} streaming={false} mode="expanded" />,
    );
    expect(document.querySelector('.thinking-body img')).toBeNull();
    expect(screen.getByText('<img src=x onerror=alert(1)>')).not.toBeNull();
  });

  it('does not show a stray preview for blank reasoning', () => {
    render(<ThinkingBlock reasoning="   " streaming={false} mode="preview" />);
    expect(document.querySelector('.thinking-preview')).toBeNull();
  });
});
