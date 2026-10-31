import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { RewindDialog } from './RewindDialog';
import type { RewindPreview } from '../api/types';
import { emptyChatState } from '../state/chat';
import { resetAppState } from '../test/fixtures';
import * as store from '../state/store';

function preview(overrides: Partial<RewindPreview> = {}): RewindPreview {
  return {
    session_id: 's_1',
    message_id: 42,
    messages_removed: 3,
    files: [],
    incomplete: false,
    opaque_tools: [],
    ...overrides,
  };
}

/** Open the dialog for the second prompt of a conversation. */
function open(target: Partial<store.RewindTarget> = {}): void {
  store.currentSessionId.value = 's_1';
  store.chat.value = emptyChatState('s_1');
  store.rewindTarget.value = {
    messageId: 42,
    preview: 'add a login page',
    isAssistant: false,
    ...target,
  };
}

describe('RewindDialog', () => {
  beforeEach(() => {
    resetAppState();
  });

  afterEach(() => {
    resetAppState();
  });

  it('is closed until a message asks for a rewind', () => {
    render(<RewindDialog loadPreview={async () => preview()} />);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('names the message and how much of the transcript goes with it', async () => {
    open();
    render(<RewindDialog loadPreview={async () => preview()} />);

    expect(screen.getByRole('dialog')).toBeTruthy();
    expect(screen.getByText('Recall to this message')).toBeTruthy();
    expect(screen.getByText('add a login page')).toBeTruthy();
    await screen.findByText('3 messages after this one will be removed.');
  });

  it('defaults to doing nothing destructive', async () => {
    open();
    const apply = vi.fn(async () => true);
    const files = preview({
      files: [
        { path: 'src/a.ts', restore: 'old', removes_file: false, changes: 1, tools: ['write_file'], incomplete: false },
      ],
    });
    render(<RewindDialog loadPreview={async () => files} apply={apply} />);

    // The revert checkbox starts unticked: restoring files overwrites whatever is
    // there now, and that must never be a surprise default.
    const revert = (await screen.findByRole('checkbox')) as HTMLInputElement;
    expect(revert.checked).toBe(false);

    fireEvent.click(screen.getByRole('button', { name: 'Rewind' }));
    await waitFor(() => expect(apply).toHaveBeenCalled());
    expect(apply).toHaveBeenCalledWith({ mode: 'none', revert: false });
  });

  it('lists the files a revert would touch, and says what happens to each', async () => {
    open();
    const files = preview({
      files: [
        {
          path: 'src/created.ts',
          save: undefined,
          removes_file: true,
          changes: 1,
          tools: ['write_file'],
          incomplete: false,
        } as unknown as RewindPreview['files'][number],
        {
          path: 'src/edited.ts',
          restore: 'before\n',
          removes_file: false,
          changes: 2,
          tools: ['edit_file'],
          incomplete: false,
        },
      ],
    });
    render(<RewindDialog loadPreview={async () => files} apply={async () => true} />);

    const revert = (await screen.findByRole('checkbox')) as HTMLInputElement;
    fireEvent.click(revert);

    expect(screen.getByText('src/created.ts')).toBeTruthy();
    expect(screen.getByText('Remove')).toBeTruthy();
    expect(screen.getByText('src/edited.ts')).toBeTruthy();
    expect(screen.getByText('Restore')).toBeTruthy();
    expect(screen.getByText('2 changes · edit_file')).toBeTruthy();
  });

  it('warns that a turn also ran tools a revert cannot undo', async () => {
    open();
    render(
      <RewindDialog
        loadPreview={async () => preview({ opaque_tools: ['run_command', 'mcp__fs__write'] })}
      />,
    );
    await screen.findByText(/cannot be undone/);
    expect(screen.getByText(/run_command, mcp__fs__write/)).toBeTruthy();
  });

  it('says when a file has no before-image to restore', async () => {
    open();
    render(
      <RewindDialog
        loadPreview={async () =>
          preview({
            incomplete: true,
            files: [
              {
                path: 'big.bin',
                removes_file: false,
                changes: 1,
                tools: ['write_file'],
                incomplete: true,
              },
            ],
          })
        }
      />,
    );
    fireEvent.click(await screen.findByRole('checkbox'));
    expect(screen.getByText(/cannot be put back exactly/)).toBeTruthy();
    expect(screen.getByText(/no saved copy of their previous/)).toBeTruthy();
  });

  it('sends a note only when one was written', async () => {
    open();
    const apply = vi.fn(async () => true);
    render(<RewindDialog loadPreview={async () => preview({ messages_removed: 0 })} apply={apply} />);

    fireEvent.click(screen.getByLabelText(/Send a note instead/));
    const submit = screen.getByRole('button', { name: 'Rewind and send note' }) as HTMLButtonElement;
    expect(submit.disabled).toBe(true);

    fireEvent.input(screen.getByRole('textbox'), {
      target: { value: 'do it the other way' },
    });
    await waitFor(() => expect(submit.disabled).toBe(false));
    fireEvent.click(submit);

    await waitFor(() => expect(apply).toHaveBeenCalled());
    expect(apply).toHaveBeenCalledWith({
      mode: 'note',
      revert: false,
      note: 'do it the other way',
    });
  });

  it('retrying sends the mode the reader picked', async () => {
    open();
    const apply = vi.fn(async () => true);
    render(<RewindDialog loadPreview={async () => preview()} apply={apply} />);

    fireEvent.click(screen.getByLabelText(/Ask again/));
    fireEvent.click(screen.getByRole('button', { name: 'Rewind and ask again' }));

    await waitFor(() => expect(apply).toHaveBeenCalled());
    expect(apply).toHaveBeenCalledWith({ mode: 'retry', revert: false });
  });

  it('a revert with no changed files is not sent as a revert', async () => {
    open();
    const apply = vi.fn(async () => true);
    render(<RewindDialog loadPreview={async () => preview()} apply={apply} />);

    // No files, so the checkbox is not even offered - and the call must not claim
    // to have restored anything.
    expect(screen.queryByRole('checkbox')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Rewind' }));
    await waitFor(() => expect(apply).toHaveBeenCalled());
    expect(apply).toHaveBeenCalledWith({ mode: 'none', revert: false });
  });

  it('keeps the dialog open and explains when nothing was applied', async () => {
    open();
    render(<RewindDialog loadPreview={async () => preview()} apply={async () => false} />);
    fireEvent.click(screen.getByRole('button', { name: 'Rewind' }));
    await screen.findByText(/The transcript is unchanged/);
    expect(screen.getByRole('dialog')).toBeTruthy();
    expect(store.rewindTarget.value).not.toBeNull();
  });

  it('resets its choices when it is opened for a different message', async () => {
    open();
    const { rerender } = render(<RewindDialog loadPreview={async () => preview()} apply={async () => true} />);
    await screen.findByText(/3 messages/);
    fireEvent.click(screen.getByLabelText(/Ask again/));

    // A different message must not inherit "ask again".
    open({ messageId: 7, preview: 'something else' });
    rerender(<RewindDialog loadPreview={async () => preview({ message_id: 7 })} apply={async () => true} />);
    await screen.findByText('something else');
    expect(screen.getByRole('button', { name: 'Rewind' })).toBeTruthy();
  });
});
