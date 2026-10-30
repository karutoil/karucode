import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import { Composer, type ComposerProps } from './Composer';
import type { Activity } from '../state/selectors';
import type { WorkspaceTree } from '../api/types';
import { configFixture, resetAppState } from '../test/fixtures';
import { emptyChatState } from '../state/chat';
import * as store from '../state/store';

const idle: Activity = { kind: 'idle', label: 'Ready' };

const workspace: WorkspaceTree = {
  path: '.',
  entries: [
    { name: 'src', path: 'src', kind: 'dir', size: 0 },
    { name: 'main.ts', path: 'src/main.ts', kind: 'file', size: 812 },
    { name: 'README.md', path: 'README.md', kind: 'file', size: 2100 },
    { name: 'plan.md', path: 'docs/my plan.md', kind: 'file', size: 40 },
    { name: 'node_modules', path: 'node_modules', kind: 'dir', size: 0 },
    { name: 'index.js', path: 'node_modules/index.js', kind: 'file', size: 10 },
  ],
};

function renderComposer(overrides: Partial<ComposerProps> = {}) {
  const props: ComposerProps = {
    disabledReason: null,
    running: false,
    stopping: false,
    status: idle,
    onSend: vi.fn(async () => true),
    onStop: vi.fn(),
    onOpenSettings: vi.fn(),
    focusTick: 0,
    loadMentionTree: vi.fn(async () => workspace),
    ...overrides,
  };
  const result = render(<Composer {...props} />);
  const textarea = screen.getByLabelText('Message karucode') as HTMLTextAreaElement;
  const fileInput = result.container.querySelector('input[type="file"]') as HTMLInputElement;
  const form = result.container.querySelector('form.composer') as HTMLFormElement;
  return { ...result, props, textarea, fileInput, form };
}

function textFile(name: string, content: string): File {
  return new File([content], name, { type: 'text/plain' });
}

function binaryFile(name: string, bytes: readonly number[] = [0, 1, 2]): File {
  return new File([new Uint8Array(bytes) as unknown as BlobPart], name, { type: 'image/png' });
}

describe('Composer', () => {
  it('uses a one-line placeholder on a phone so the empty dock stays one line', () => {
    // At 16px (the iOS no-zoom size) the desktop sentence wraps to two lines in a
    // ~390px column, which made an empty composer stand two lines tall.
    const matchMedia = window.matchMedia;
    try {
      window.matchMedia = ((query: string) => ({
        matches: query.includes('640px'),
        media: query,
        onchange: null,
        addListener: () => undefined,
        removeListener: () => undefined,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
        dispatchEvent: () => false,
      })) as typeof window.matchMedia;

      const { textarea } = renderComposer();
      expect(textarea.getAttribute('placeholder')).toBe('Ask karucode anything…');
      // The draft itself can still be long; only the hint is shortened.
      fireEvent.input(textarea, { target: { value: 'Explain the build' } });
      expect(textarea.value).toBe('Explain the build');
    } finally {
      window.matchMedia = matchMedia;
    }
  });

  it('keeps the full placeholder on a desktop viewport', () => {
    const { textarea } = renderComposer();
    expect(textarea.getAttribute('placeholder')).toBe(
      'Ask karucode anything, or describe a change...',
    );
  });

  it('sends with Cmd/Ctrl+Enter and clears the draft on success', async () => {
    const { props, textarea } = renderComposer();
    fireEvent.input(textarea, { target: { value: 'Explain the build' } });
    fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true });

    await waitFor(() => expect(props.onSend).toHaveBeenCalledWith('Explain the build'));
    await waitFor(() => expect(textarea.value).toBe(''));
  });

  it('keeps the draft when the send is rejected', async () => {
    const { props, textarea } = renderComposer({ onSend: vi.fn(async () => false) });
    fireEvent.input(textarea, { target: { value: 'Keep me' } });
    fireEvent.keyDown(textarea, { key: 'Enter', metaKey: true });

    await waitFor(() => expect(props.onSend).toHaveBeenCalled());
    expect(textarea.value).toBe('Keep me');
  });

  it('sends with the Send button and trims only for the emptiness check', async () => {
    const { props, textarea } = renderComposer();
    fireEvent.input(textarea, { target: { value: '  hello  ' } });
    fireEvent.click(screen.getByRole('button', { name: /Send message/ }));
    await waitFor(() => expect(props.onSend).toHaveBeenCalledWith('  hello  '));
  });

  it('shows Stop while running, stops on click and on Escape', () => {
    const { props, textarea } = renderComposer({
      running: true,
      status: { kind: 'streaming', label: 'Streaming' },
    });
    expect(screen.queryByRole('button', { name: /Send message/ })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /Stop run/ }));
    expect(props.onStop).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(textarea, { key: 'Escape' });
    expect(props.onStop).toHaveBeenCalledTimes(2);
  });

  it('disables typing with an explanation when there is no provider', () => {
    const { props, textarea } = renderComposer({
      disabledReason: 'No model provider configured. Add one in Settings.',
    });
    expect(textarea.disabled).toBe(true);
    const send = screen.getByRole('button', { name: /Send message/ }) as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    expect((screen.getByRole('button', { name: 'Attach files' }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(/No model provider configured/)).not.toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Open settings' }));
    expect(props.onOpenSettings).toHaveBeenCalledTimes(1);
  });

  it('shows a character counter after 4000 characters', () => {
    const { textarea } = renderComposer();
    fireEvent.input(textarea, { target: { value: 'x'.repeat(4001) } });
    expect(screen.getByText(/4,001 characters/)).not.toBeNull();
  });

  it('announces the activity in a polite live region', () => {
    renderComposer({ status: { kind: 'tool', label: 'Running read_file' } });
    const status = screen.getByRole('status');
    expect(status.getAttribute('aria-live')).toBe('polite');
    expect(status.textContent).toContain('Running read_file');
  });

  it('keeps Send disabled with an empty draft', () => {
    renderComposer();
    expect((screen.getByRole('button', { name: /Send message/ }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  describe('mock fidelity', () => {
    afterEach(() => {
      store.config.value = null;
      resetAppState();
    });

    it('keeps the footnote outside the input box', () => {
      const { container, form } = renderComposer();
      const foot = container.querySelector('.compose-foot') as HTMLElement;
      expect(foot).not.toBeNull();
      // Mock: `.composer-region > (.composer + .compose-foot)`.
      expect(form.contains(foot)).toBe(false);
      expect(foot.parentElement).toBe(form.parentElement);
    });

    it('draws the mock arrow glyph on Send', () => {
      const { textarea } = renderComposer();
      fireEvent.input(textarea, { target: { value: 'hello' } });
      const send = screen.getByRole('button', { name: /Send message/ });
      const path = send.querySelector('svg path');
      expect(path?.getAttribute('d')).toBe('M5 12h14m-6-6 6 6-6 6');
      expect(send.querySelector('svg')?.getAttribute('width')).toBe('15');
    });

    it('reads the approval policy into the right-hand footnote zone', () => {
      store.config.value = configFixture;
      renderComposer();
      const approvals = document.querySelector('.compose-approvals') as HTMLElement;
      expect(approvals).not.toBeNull();
      expect(approvals.textContent).toContain('Approvals: ask');
      expect(approvals.getAttribute('title')).toContain('1 allowlisted command');
    });

    it('keeps the token usage reachable when it leaves the footnote', () => {
      renderComposer({ usageLabel: '1,234 tokens in · 567 out' });
      const status = screen.getByRole('status');
      expect(status.textContent).not.toContain('1,234 tokens');
      expect(status.getAttribute('title')).toContain('1,234 tokens in · 567 out');
    });

    describe('context window pill', () => {
      /** The composer's live state, as the store would hold it after a call. */
      const withUsage = (lastPromptTokens: number | null) => {
        store.chat.value = { ...emptyChatState('s_ab12'), lastPromptTokens };
      };

      it('shows the window fill in the bottom row once a call reports usage', () => {
        store.config.value = configFixture;
        withUsage(32_000);
        const { container } = renderComposer();
        const pill = container.querySelector('.context-pill') as HTMLElement;
        expect(pill).not.toBeNull();
        // configFixture's model declares context_window: 128_000.
        expect(pill.textContent).toBe('25.0%');
        expect(pill.getAttribute('title')).toContain('32,000 of 128,000 context tokens');
        expect(pill.getAttribute('aria-label')).toContain('25.0% full');
      });

      it('renders nothing before the first call, rather than 0%', () => {
        store.config.value = configFixture;
        withUsage(null);
        const { container } = renderComposer();
        expect(container.querySelector('.context-pill')).toBeNull();
      });

      it('renders nothing when the model declares no window', () => {
        store.config.value = {
          ...configFixture,
          providers: [
            {
              ...configFixture.providers[0]!,
              models: [{ id: 'gpt-4o-mini', name: 'GPT-4o mini' }],
            },
          ],
        };
        withUsage(32_000);
        const { container } = renderComposer();
        expect(container.querySelector('.context-pill')).toBeNull();
      });

      it('follows the session pin, not the install default', () => {
        // The pinned model is the one a run would use, so it owns the window.
        store.config.value = {
          ...configFixture,
          providers: [
            {
              ...configFixture.providers[0]!,
              models: [
                { id: 'gpt-4o-mini', name: 'GPT-4o mini', context_window: 128_000 },
                { id: 'big', name: 'Big', context_window: 1_000_000 },
              ],
            },
          ],
        };
        store.sessions.value = [
          {
            id: 's_ab12',
            title: 'Session',
            workspace: '/home/me/project',
            project_id: 'p_1',
            model: { provider: 'default', model: 'big' },
            created_at: 1,
            updated_at: 2,
            message_count: 0,
            last_run_status: null,
            running: false,
          },
        ];
        store.currentSessionId.value = 's_ab12';
        withUsage(250_000);
        const { container } = renderComposer();
        expect(container.querySelector('.context-pill')?.textContent).toBe('25.0%');
      });

      it('warns before the window is exhausted', () => {
        store.config.value = configFixture;
        withUsage(115_200); // 90% of 128k
        const { container } = renderComposer();
        expect(container.querySelector('.context-pill')?.getAttribute('data-tone')).toBe('hot');

        withUsage(94_000); // 73%
        const second = renderComposer();
        expect(second.container.querySelector('.context-pill')?.getAttribute('data-tone')).toBe('warn');
        second.unmount();
      });
    });

    it('writes the shortcut hint as plain text, not keycaps', () => {
      const { container } = renderComposer();
      const hint = container.querySelector('.composer-hint') as HTMLElement;
      expect(hint.textContent?.replace(/\s+/g, ' ').trim()).toBe('Ctrl Enter to send·@ mention files');
      expect(hint.querySelector('kbd')).toBeNull();
    });

    it('takes a draft handed over after mount and focuses the box', () => {
      const { textarea } = renderComposer();
      // The empty state's quick prompts write the signal while the composer is
      // already on screen, so the hand-off cannot be mount-only.
      act(() => {
        store.composerDraft.value = 'Explore the codebase and suggest improvements.';
      });
      expect(textarea.value).toBe('Explore the codebase and suggest improvements.');
      expect(document.activeElement).toBe(textarea);
      expect(store.composerDraft.value).toBeNull();
    });
  });

  describe('@-mentions', () => {
    it('lists real workspace paths and inserts the chosen one on Enter', async () => {
      const { props, textarea } = renderComposer();
      fireEvent.input(textarea, { target: { value: 'look at @main.ts' } });

      const listbox = await screen.findByRole('listbox');
      expect(within(listbox).getByRole('option', { name: /main\.ts/ })).toBeTruthy();
      // Noise directories never reach the menu.
      expect(within(listbox).queryByText(/index\.js/)).toBeNull();

      fireEvent.keyDown(textarea, { key: 'Enter' });
      await waitFor(() => expect(textarea.value).toBe('look at @src/main.ts '));
      expect(screen.queryByRole('listbox')).toBeNull();
      expect(props.loadMentionTree).toHaveBeenCalledWith(6);
    });

    it('exposes combobox semantics while the menu is open, then closes them', async () => {
      const { textarea } = renderComposer();
      expect(textarea.getAttribute('aria-expanded')).toBe('false');
      expect(textarea.getAttribute('aria-controls')).toBeNull();

      fireEvent.input(textarea, { target: { value: '@src' } });
      await screen.findByRole('listbox');
      expect(textarea.getAttribute('role')).toBe('combobox');
      expect(textarea.getAttribute('aria-expanded')).toBe('true');
      expect(textarea.getAttribute('aria-autocomplete')).toBe('list');
      const controls = textarea.getAttribute('aria-controls');
      expect(controls).toBe('composer-mention-menu');
      expect(document.getElementById(controls as string)).not.toBeNull();
      expect(textarea.getAttribute('aria-activedescendant')).toBe('composer-mention-menu-option-0');
    });

    it('moves the highlight with the arrow keys and inserts that option', async () => {
      const { textarea } = renderComposer();
      fireEvent.input(textarea, { target: { value: '@' } });
      await screen.findByRole('listbox');

      const options = screen.getAllByRole('option');
      expect(options.length).toBeGreaterThan(1);
      const second = options[1] as HTMLElement;
      const path = second.getAttribute('title') as string;

      fireEvent.keyDown(textarea, { key: 'ArrowDown' });
      expect(second.getAttribute('aria-selected')).toBe('true');
      fireEvent.keyDown(textarea, { key: 'Enter' });

      await waitFor(() => expect(textarea.value).toBe(`@"${path}" `));
    });

    it('inserts a quoted mention for a path with spaces', async () => {
      const { textarea } = renderComposer();
      // Quoted mentions are the way to reference a path containing spaces.
      fireEvent.input(textarea, { target: { value: 'read @"my plan' } });
      const listbox = await screen.findByRole('listbox');
      fireEvent.click(within(listbox).getByRole('option', { name: /my plan\.md/ }));
      await waitFor(() => expect(textarea.value).toBe('read @"docs/my plan.md" '));
    });

    it('closes the menu on Escape without touching the draft', async () => {
      const { textarea } = renderComposer();
      fireEvent.input(textarea, { target: { value: 'hey @src' } });
      await screen.findByRole('listbox');
      fireEvent.keyDown(textarea, { key: 'Escape' });
      await waitFor(() => expect(screen.queryByRole('listbox')).toBeNull());
      expect(textarea.value).toBe('hey @src');
    });

    it('styles the popover like the mock menus: title row, plain-text footer', async () => {
      const { textarea, container } = renderComposer();
      fireEvent.input(textarea, { target: { value: '@src' } });
      await screen.findByRole('listbox');
      const head = container.querySelector('.mention-head') as HTMLElement;
      expect(head.textContent).toContain('Files in this workspace');
      const foot = container.querySelector('.mention-foot') as HTMLElement;
      expect(foot.textContent).toContain('to move');
      expect(foot.querySelector('kbd')).toBeNull();
    });

    it('stays usable and explains itself when indexing fails', async () => {
      const { textarea } = renderComposer({
        loadMentionTree: vi.fn(async () => {
          throw new Error('offline');
        }),
      });
      fireEvent.input(textarea, { target: { value: '@src' } });
      expect(await screen.findByText(/Could not index the workspace: offline/)).not.toBeNull();

      // The draft is untouched and Send still works.
      fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true });
      await waitFor(() => expect(textarea.value).toBe(''));
    });
  });

  describe('attachments', () => {
    it('inlines a small text file and puts it in the send payload', async () => {
      const { props, textarea, fileInput } = renderComposer();
      fireEvent.change(fileInput, { target: { files: [textFile('notes.md', '# Note\nbody')] } });

      expect(await screen.findByText('notes.md')).not.toBeNull();
      await waitFor(() => expect(screen.getByText(/Inlined · 11 B/)).not.toBeNull());

      fireEvent.input(textarea, { target: { value: 'Summarise this' } });
      fireEvent.click(screen.getByRole('button', { name: /Send message/ }));

      await waitFor(() => expect(props.onSend).toHaveBeenCalledTimes(1));
      const payload = (props.onSend as unknown as { mock: { calls: string[][] } }).mock.calls[0]?.[0] as string;
      expect(payload).toContain('Summarise this');
      expect(payload).toContain('### notes.md (inlined, 11 B)');
      expect(payload).toContain('# Note');
      expect(payload).toContain('body');
    });

    it('uploads a binary file and references its workspace path', async () => {
      const uploadFile = vi.fn(async () => ({
        path: '.karucode/attachments/s_1/shot.png',
      }));
      const { props, textarea, fileInput } = renderComposer({ uploadFile });
      fireEvent.change(fileInput, { target: { files: [binaryFile('shot.png')] } });

      expect(await screen.findByText('shot.png')).not.toBeNull();
      await waitFor(() => expect(uploadFile).toHaveBeenCalledTimes(1));
      await waitFor(() => expect(screen.getByText(/Uploaded · 3 B/)).not.toBeNull());

      fireEvent.input(textarea, { target: { value: 'What is in this image?' } });
      fireEvent.click(screen.getByRole('button', { name: /Send message/ }));
      await waitFor(() => expect(props.onSend).toHaveBeenCalledTimes(1));
      const payload = (props.onSend as unknown as { mock: { calls: string[][] } }).mock.calls[0]?.[0] as string;
      expect(payload).toContain('.karucode/attachments/s_1/shot.png');
      expect(payload).toContain('(uploaded, 3 B)');
    });

    it('references the stored name the server chose, not the local file name', async () => {
      // The contract says names are never overwritten: report.pdf -> report-1.pdf.
      const uploadFile = vi.fn(async () => ({
        path: '.karucode/attachments/s_1/report-1.pdf',
      }));
      const { props, textarea, fileInput } = renderComposer({ uploadFile });
      const pdf = new File([new Uint8Array([37, 80, 68, 70])], 'report.pdf', {
        type: 'application/pdf',
      });
      fireEvent.change(fileInput, { target: { files: [pdf] } });
      await waitFor(() => expect(uploadFile).toHaveBeenCalledTimes(1));
      await waitFor(() => expect(screen.getByText(/Uploaded · 4 B/)).not.toBeNull());

      // The chip still names the file the user picked, and shows where it went.
      const chip = screen.getByText('report.pdf').closest('.attachment-chip') as HTMLElement;
      expect(chip.getAttribute('data-mode')).toBe('upload');
      expect(screen.getByTitle('.karucode/attachments/s_1/report-1.pdf')).not.toBeNull();

      fireEvent.input(textarea, { target: { value: 'Read the report' } });
      fireEvent.click(screen.getByRole('button', { name: /Send message/ }));
      await waitFor(() => expect(props.onSend).toHaveBeenCalledTimes(1));
      const payload = (props.onSend as unknown as { mock: { calls: string[][] } }).mock.calls[0]?.[0] as string;
      expect(payload).toContain('### .karucode/attachments/s_1/report-1.pdf (uploaded, 4 B)');
      expect(payload).not.toContain('### report.pdf');
    });

    it('blocks sending while an upload is still running', async () => {
      const gate: { resolve: ((value: { path: string }) => void) | null } = { resolve: null };
      const uploadFile = vi.fn(
        () =>
          new Promise<{ path: string }>((done) => {
            gate.resolve = done;
          }),
      );
      const { textarea, fileInput } = renderComposer({ uploadFile });
      fireEvent.change(fileInput, { target: { files: [binaryFile('shot.png')] } });
      fireEvent.input(textarea, { target: { value: 'hi' } });

      await waitFor(() => expect(screen.getByText(/Uploading/)).not.toBeNull());
      expect((screen.getByRole('button', { name: /Send message/ }) as HTMLButtonElement).disabled).toBe(
        true,
      );

      gate.resolve?.({ path: '.karucode/attachments/s_1/shot.png' });
      await waitFor(() =>
        expect((screen.getByRole('button', { name: /Send message/ }) as HTMLButtonElement).disabled).toBe(
          false,
        ),
      );
    });

    it('shows the failure and keeps Send disabled when the upload fails', async () => {
      const uploadFile = vi.fn(async () => {
        throw new Error('disk full');
      });
      const { fileInput } = renderComposer({ uploadFile });
      fireEvent.change(fileInput, { target: { files: [binaryFile('shot.png')] } });

      expect(await screen.findByText('disk full')).not.toBeNull();
      const chip = screen.getByRole('list', { name: 'Attached files' });
      expect(chip.querySelector('.attachment-chip-error')).not.toBeNull();
    });

    it('removes a chip without sending', async () => {
      const { props, fileInput } = renderComposer();
      fireEvent.change(fileInput, { target: { files: [textFile('notes.md', 'hello')] } });
      await screen.findByText('notes.md');
      fireEvent.click(screen.getByRole('button', { name: 'Remove notes.md' }));
      await waitFor(() => expect(screen.queryByText('notes.md')).toBeNull());
      expect(props.onSend).not.toHaveBeenCalled();
    });

    it('accepts files dropped on the composer', async () => {
      const { form } = renderComposer();
      fireEvent.drop(form, {
        dataTransfer: { files: [textFile('dropped.md', 'hi there')], types: ['Files'] },
      });
      expect(await screen.findByText('dropped.md')).not.toBeNull();
    });

    it('shows the drop overlay while files are dragged over the composer', () => {
      const { form } = renderComposer();
      fireEvent.dragEnter(form, { dataTransfer: { types: ['Files'], files: [] } });
      expect(screen.getByText('Drop files to attach')).not.toBeNull();
      fireEvent.dragLeave(form, { dataTransfer: { types: ['Files'], files: [] } });
      expect(screen.queryByText('Drop files to attach')).toBeNull();
    });

    it('ignores a drag that carries no files', () => {
      const { form } = renderComposer();
      fireEvent.dragEnter(form, { dataTransfer: { types: ['text/plain'], files: [] } });
      expect(screen.queryByText('Drop files to attach')).toBeNull();
    });

    it('rejects a file past the upload ceiling without calling the server', async () => {
      const uploadFile = vi.fn(async () => ({ path: 'unused' }));
      const { fileInput } = renderComposer({ uploadFile });
      const huge = binaryFile('huge.bin');
      Object.defineProperty(huge, 'size', { value: 9 * 1024 * 1024 });
      fireEvent.change(fileInput, { target: { files: [huge] } });

      expect(await screen.findByText(/Larger than 8.0 MB/)).not.toBeNull();
      expect(uploadFile).not.toHaveBeenCalled();
    });
  });

  describe('per-session draft', () => {
    it('does not carry a half-written message into another session', async () => {
      store.currentSessionId.value = 's_ab12';
      const { textarea } = renderComposer();

      fireEvent.input(textarea, { target: { value: 'half-written for A' } });
      expect(textarea.value).toBe('half-written for A');

      // Switching sessions keeps the composer mounted, so the draft must be
      // parked rather than shown in the other conversation.
      await act(async () => {
        store.currentSessionId.value = 's_cd34';
      });
      expect(textarea.value).toBe('');

      // Coming back restores what was typed for that session.
      await act(async () => {
        store.currentSessionId.value = 's_ab12';
      });
      expect(textarea.value).toBe('half-written for A');
    });

    it('keeps each session draft separate', async () => {
      store.currentSessionId.value = 's_ab12';
      const { textarea } = renderComposer();

      fireEvent.input(textarea, { target: { value: 'draft A' } });
      await act(async () => {
        store.currentSessionId.value = 's_cd34';
      });
      fireEvent.input(textarea, { target: { value: 'draft B' } });

      await act(async () => {
        store.currentSessionId.value = 's_ab12';
      });
      expect(textarea.value).toBe('draft A');

      await act(async () => {
        store.currentSessionId.value = 's_cd34';
      });
      expect(textarea.value).toBe('draft B');
    });
  });

  describe('mention index invalidation', () => {
    it('re-indexes the workspace after a project switch', async () => {
      store.currentProjectId.value = 'p_one';
      const loadMentionTree = vi.fn(async () => workspace);
      const { textarea } = renderComposer({ loadMentionTree });

      fireEvent.input(textarea, { target: { value: '@main' } });
      await waitFor(() => expect(loadMentionTree).toHaveBeenCalledTimes(1));

      // The index is per project: the 60 s TTL must not keep the old file list.
      await act(async () => {
        store.currentProjectId.value = 'p_two';
      });

      await waitFor(() => expect(loadMentionTree).toHaveBeenCalledTimes(2));
    });
  });
});