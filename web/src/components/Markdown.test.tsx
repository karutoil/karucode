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

  /* The button's accessible name is its `aria-label`, so changing the visible
   * text announces nothing. The status is also written to a live region, which is
   * the only way a screen reader hears the outcome. */
  it('announces a successful copy through a live region', async () => {
    vi.useFakeTimers();
    vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue(undefined);
    render(<Markdown text={SOURCE} />);
    const status = screen.getByTestId('markdown-copy-status');
    expect(status.getAttribute('aria-live')).toBe('polite');
    expect(status.textContent).toBe('');

    fireEvent.click(copyButtons()[0] as HTMLButtonElement);
    await act(async () => {
      await Promise.resolve();
    });
    expect(status.textContent).toContain('copied to the clipboard');

    // The region clears with the label, so the next copy is a new announcement.
    await act(async () => {
      vi.advanceTimersByTime(1500);
    });
    expect(status.textContent).toBe('');
  });

  it('announces a rejected copy, with what to do instead', async () => {
    vi.useFakeTimers();
    vi.spyOn(navigator.clipboard, 'writeText').mockRejectedValue(new Error('denied'));
    render(<Markdown text={SOURCE} />);
    const status = screen.getByTestId('markdown-copy-status');

    fireEvent.click(copyButtons()[0] as HTMLButtonElement);
    await act(async () => {
      await Promise.resolve();
    });

    expect(status.textContent).toContain('Copy failed');
    expect(status.textContent).toContain('copy it manually');
    // The state is also on the button, so a passing glance sees it.
    expect(copyButtons()[0]?.dataset.copyState).toBe('failed');
  });
});