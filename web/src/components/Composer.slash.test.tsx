import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/preact';
import { Composer, type ComposerProps } from './Composer';
import type { Activity } from '../state/selectors';
import type { CommandSpec } from '../api/types';
import { resetAppState } from '../test/fixtures';
import * as store from '../state/store';

const idle: Activity = { kind: 'idle', label: 'Ready' };

function textFile(name: string, content: string): File {
  return new File([content], name, { type: 'text/plain' });
}

const CATALOG: CommandSpec[] = [
  {
    name: 'help',
    title: 'List every command',
    summary: 'What each slash command does',
    args: '[command]',
    category: 'meta',
    source: 'builtin',
    insert: '/help ',
  },
  {
    name: 'compact',
    title: 'Compact the conversation',
    summary: 'Fold the older turns into one digest now',
    args: '',
    category: 'context',
    source: 'builtin',
    insert: '/compact',
  },
  {
    name: 'model',
    title: 'Show or pin the model',
    summary: 'Pin this session to one model',
    args: '[provider/model]',
    category: 'session',
    source: 'builtin',
    insert: '/model ',
  },
];

function renderComposer(overrides: Partial<ComposerProps> = {}) {
  const props: ComposerProps = {
    disabledReason: null,
    running: false,
    stopping: false,
    status: idle,
    onSend: vi.fn(async () => true),
    onCommand: vi.fn(async () => true),
    onStop: vi.fn(),
    onOpenSettings: vi.fn(),
    focusTick: 0,
    loadCommands: vi.fn(async () => CATALOG),
    ...overrides,
  };
  const result = render(<Composer {...props} />);
  const textarea = screen.getByLabelText('Message karucode') as HTMLTextAreaElement;
  const fileInput = result.container.querySelector('input[type="file"]') as HTMLInputElement;
  return { ...result, props, textarea, fileInput };
}

describe('the composer slash menu', () => {
  beforeEach(() => {
    resetAppState();
    store.currentProjectId.value = 'p_1';
  });

  it('opens on `/` and lists the catalog in server order', async () => {
    const { props, textarea } = renderComposer();
    fireEvent.input(textarea, { target: { value: '/' } });

    const listbox = await screen.findByRole('listbox');
    expect(listbox).toBeTruthy();
    expect(await screen.findByText('Compact the conversation')).toBeTruthy();
    expect(await screen.findByText('Show or pin the model')).toBeTruthy();
    expect(props.loadCommands).toHaveBeenCalledWith('p_1');
  });

  it('filters as the name is typed', async () => {
    const { textarea } = renderComposer();
    fireEvent.input(textarea, { target: { value: '/com' } });

    await screen.findByRole('listbox');
    await waitFor(() => {
      expect(screen.queryByText('Show or pin the model')).toBeNull();
    });
    expect(screen.getByText('Compact the conversation')).toBeTruthy();
  });

  it('inserts the chosen command with its trailing space and closes the menu', async () => {
    const { props, textarea } = renderComposer();
    fireEvent.input(textarea, { target: { value: '/mo' } });

    await screen.findByRole('listbox');
    // Enter picks the highlighted row instead of sending the half-typed command.
    fireEvent.keyDown(textarea, { key: 'Enter' });

    await waitFor(() => expect(textarea.value).toBe('/model '));
    expect(props.onSend).not.toHaveBeenCalled();
    expect(props.onCommand).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull());
  });

  it('walks the list with the arrow keys', async () => {
    const { textarea } = renderComposer();
    fireEvent.input(textarea, { target: { value: '/' } });

    await screen.findByRole('listbox');
    fireEvent.keyDown(textarea, { key: 'ArrowDown' });
    fireEvent.keyDown(textarea, { key: 'Enter' });

    // The second row of the catalog is `/compact`, which takes no arguments.
    await waitFor(() => expect(textarea.value).toBe('/compact'));
  });

  it('dismisses on Escape without touching the draft', async () => {
    const { textarea } = renderComposer();
    fireEvent.input(textarea, { target: { value: '/hel' } });

    await screen.findByRole('listbox');
    fireEvent.keyDown(textarea, { key: 'Escape' });

    await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull());
    expect(textarea.value).toBe('/hel');
  });

  it('does not open for a message that merely starts with a slash', async () => {
    const { textarea } = renderComposer();
    fireEvent.input(textarea, { target: { value: '/home/toil/notes.md' } });

    // Nothing to list, and no listbox: this is a path, not a command.
    expect(screen.queryByRole('listbox')).toBeNull();
  });

  it('runs a whole command through onCommand instead of sending a prompt', async () => {
    const { props, textarea } = renderComposer();
    fireEvent.input(textarea, { target: { value: '/' } });
    await screen.findByRole('listbox');
    // The catalog is what decides the send path, so wait for it to land.
    await screen.findByText('Compact the conversation');

    fireEvent.input(textarea, { target: { value: '/compact' } });
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true });

    await waitFor(() => expect(props.onCommand).toHaveBeenCalledWith('/compact'));
    expect(props.onSend).not.toHaveBeenCalled();
    await waitFor(() => expect(textarea.value).toBe(''));
  });

  it('refuses to run a command with an attachment instead of losing the file', async () => {
    // The text after a command is its arguments, so an attachment would be read as
    // arguments and never reach the model - and the command's reply clears the
    // composer, taking the file with it. Enter used to fall through to the ordinary
    // send path here and the file was gone with nothing said.
    const { props, textarea, fileInput } = renderComposer();
    fireEvent.input(textarea, { target: { value: '/' } });
    await screen.findByText('Compact the conversation');

    fireEvent.change(fileInput, { target: { files: [textFile('notes.md', '# Note')] } });
    await screen.findByText('notes.md');

    fireEvent.input(textarea, { target: { value: '/compact' } });
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true });

    await waitFor(() => expect(screen.getByText(/cannot carry attachments/)).toBeTruthy());
    expect(props.onCommand).not.toHaveBeenCalled();
    expect(props.onSend).not.toHaveBeenCalled();
    expect(textarea.value).toBe('/compact');
    expect(screen.getByText('notes.md')).toBeTruthy();
  });

  it('runs a command while a run is in flight instead of queueing it', async () => {
    // `/stop` is the reason the branch exists: a command must never wait behind
    // the run it is trying to change.
    const onQueue = vi.fn(async () => true);
    const { props, textarea } = renderComposer({ running: true, onQueue });
    fireEvent.input(textarea, { target: { value: '/' } });
    await screen.findByText('Compact the conversation');

    fireEvent.input(textarea, { target: { value: '/compact' } });
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true });

    await waitFor(() => expect(props.onCommand).toHaveBeenCalledWith('/compact'));
    expect(onQueue).not.toHaveBeenCalled();
  });

  it('labels the send button with the command it will run', async () => {
    const { textarea } = renderComposer();
    fireEvent.input(textarea, { target: { value: '/' } });
    await screen.findByText('Compact the conversation');

    fireEvent.input(textarea, { target: { value: '/compact' } });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /Run \/compact/ })).toBeTruthy(),
    );
  });

  it('falls back to onSend when the caller wired no command handler', async () => {
    const { props, textarea } = renderComposer({ onCommand: undefined });
    fireEvent.input(textarea, { target: { value: '/' } });
    await screen.findByText('Compact the conversation');

    fireEvent.input(textarea, { target: { value: '/compact' } });
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true });

    await waitFor(() => expect(props.onSend).toHaveBeenCalledWith('/compact'));
  });
});