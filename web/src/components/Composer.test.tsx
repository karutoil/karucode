import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { Composer } from './Composer';
import type { Activity } from '../state/selectors';

const idle: Activity = { kind: 'idle', label: 'Ready' };

function renderComposer(overrides: Partial<Parameters<typeof Composer>[0]> = {}) {
  const props = {
    disabledReason: null,
    running: false,
    stopping: false,
    status: idle,
    onSend: vi.fn(async () => true),
    onStop: vi.fn(),
    onOpenSettings: vi.fn(),
    focusTick: 0,
    ...overrides,
  };
  const result = render(<Composer {...props} />);
  return { ...result, props };
}

describe('Composer', () => {
  it('sends with Cmd/Ctrl+Enter and clears the draft on success', async () => {
    const { props } = renderComposer();
    const textarea = screen.getByLabelText('Message karucode') as HTMLTextAreaElement;
    fireEvent.input(textarea, { target: { value: 'Explain the build' } });
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true });

    await waitFor(() => expect(props.onSend).toHaveBeenCalledWith('Explain the build'));
    await waitFor(() => expect(textarea.value).toBe(''));
  });

  it('keeps the draft when the send is rejected', async () => {
    const { props } = renderComposer({ onSend: vi.fn(async () => false) });
    const textarea = screen.getByLabelText('Message karucode') as HTMLTextAreaElement;
    fireEvent.input(textarea, { target: { value: 'Keep me' } });
    fireEvent.keyDown(textarea, { key: 'Enter', metaKey: true });

    await waitFor(() => expect(props.onSend).toHaveBeenCalled());
    expect(textarea.value).toBe('Keep me');
  });

  it('sends with the Send button and trims only for the emptiness check', async () => {
    const { props } = renderComposer();
    const textarea = screen.getByLabelText('Message karucode') as HTMLTextAreaElement;
    fireEvent.input(textarea, { target: { value: '  hello  ' } });
    fireEvent.click(screen.getByRole('button', { name: /Send message/ }));
    await waitFor(() => expect(props.onSend).toHaveBeenCalledWith('  hello  '));
  });

  it('shows Stop while running, stops on click and on Escape', () => {
    const { props } = renderComposer({
      running: true,
      status: { kind: 'streaming', label: 'Streaming' },
    });
    expect(screen.queryByRole('button', { name: /Send message/ })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /Stop run/ }));
    expect(props.onStop).toHaveBeenCalledTimes(1);

    const textarea = screen.getByLabelText('Message karucode');
    fireEvent.keyDown(textarea, { key: 'Escape' });
    expect(props.onStop).toHaveBeenCalledTimes(2);
  });

  it('disables typing with an explanation when there is no provider', () => {
    const { props } = renderComposer({
      disabledReason: 'No model provider configured. Add one in Settings.',
    });
    const textarea = screen.getByLabelText('Message karucode') as HTMLTextAreaElement;
    expect(textarea.disabled).toBe(true);
    const send = screen.getByRole('button', { name: /Send message/ }) as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    expect(screen.getByText(/No model provider configured/)).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Open settings' }));
    expect(props.onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it('shows a character counter after 4000 characters', () => {
    renderComposer();
    const textarea = screen.getByLabelText('Message karucode');
    fireEvent.input(textarea, { target: { value: 'x'.repeat(4001) } });
    expect(screen.getByText(/4,001 characters/)).not.toBeNull();
  });

  it('announces the activity in a polite live region', () => {
    renderComposer({ status: { kind: 'tool', label: 'Running read_file' } });
    const status = screen.getByRole('status');
    expect(status.getAttribute('aria-live')).toBe('polite');
    expect(status.textContent).toContain('Running read_file');
  });
});