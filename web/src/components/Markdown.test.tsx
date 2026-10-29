import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/preact';
import { Markdown } from './Markdown';

const SOURCE = 'First\n\n```rust\nfn one() {}\n```\n\nThen\n\n```ts\nconst two = 2;\n```\n';

function copyButtons(): HTMLButtonElement[] {
  return Array.from(document.querySelectorAll<HTMLButtonElement>('button[data-copy]'));
}

describe('Markdown copy buttons', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('copies the code of the block it belongs to, not the first block', async () => {
    const writeText = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue(undefined);
    render(<Markdown text={SOURCE} />);

    const buttons = copyButtons();
    expect(buttons).toHaveLength(2);
    fireEvent.click(buttons[1] as HTMLButtonElement);

    expect(writeText).toHaveBeenCalledWith('const two = 2;\n');
  });

  it('confirms the copy, then returns to the idle label', async () => {
    vi.useFakeTimers();
    vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue(undefined);
    render(<Markdown text={SOURCE} />);
    const button = copyButtons()[0] as HTMLButtonElement;

    fireEvent.click(button);
    await act(async () => {
      await Promise.resolve();
    });
    expect(button.textContent).toBe('Copied');

    await act(async () => {
      vi.advanceTimersByTime(1500);
    });
    expect(button.textContent).toBe('Copy');
    expect(screen.getAllByText('Copy')).toHaveLength(2);
  });

  it('says so when the clipboard is unavailable', async () => {
    vi.useFakeTimers();
    vi.spyOn(navigator.clipboard, 'writeText').mockRejectedValue(new Error('denied'));
    render(<Markdown text={SOURCE} />);
    const button = copyButtons()[0] as HTMLButtonElement;

    fireEvent.click(button);
    await act(async () => {
      await Promise.resolve();
    });
    expect(button.textContent).toBe('Copy failed');
  });
});