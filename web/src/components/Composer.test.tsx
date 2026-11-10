import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/preact';
import { Composer, type ComposerProps } from './Composer';
import type { Activity } from '../state/selectors';
import type { WorkspaceTree } from '../api/types';
import { configFixture, resetAppState } from '../test/fixtures';
import { emptyChatState } from '../state/chat';
import * as store from '../state/store';
import * as workbench from '../state/workbench';
import { describeApprovals } from '../lib/approvalCopy';

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

  it('retains draft-driven auto-grow and re-measures wrapping after a resize', async () => {
    const { textarea } = renderComposer();
    let measuredHeight = 44;
    Object.defineProperty(textarea, 'scrollHeight', {
      configurable: true,
      get: () => measuredHeight,
    });

    measuredHeight = 92;
    fireEvent.input(textarea, { target: { value: 'first line\nsecond line' } });
    await waitFor(() => expect(textarea.style.height).toBe('92px'));

    // A narrower viewport can wrap the unchanged draft onto another line. Do not
    // retain a stale inline height just because the draft dependency did not change.
    measuredHeight = 108;
    fireEvent(window, new Event('resize'));
    await waitFor(() => expect(textarea.style.height).toBe('108px'));
    expect(textarea.value).toBe('first line\nsecond line');

    measuredHeight = 44;
    fireEvent.input(textarea, { target: { value: 'one line' } });
    await waitFor(() => expect(textarea.style.height).toBe('44px'));
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
      workbench.closeContext();
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

    it('draws the mock up-arrow glyph in the round Send button', () => {
      const { textarea } = renderComposer();
      fireEvent.input(textarea, { target: { value: 'hello' } });
      const send = screen.getByRole('button', { name: /Send message/ });
      // Mock: `#i-arrow-up` — a bare up-arrow; the label is the accessible name,
      // not visible text.
      const path = send.querySelector('svg path');
      expect(path?.getAttribute('d')).toBe('M12 19V5M6 11l6-6 6 6');
      expect(send.querySelector('svg')?.getAttribute('width')).toBe('14');
      expect(send.textContent).toBe('');
    });

    it('keeps the attach control in the bottom bar, like the mock', () => {
      const { container } = renderComposer();
      const bar = container.querySelector('.compose-bottom') as HTMLElement;
      const attach = screen.getByRole('button', { name: 'Attach files' });
      expect(bar.contains(attach)).toBe(true);
      // The mock's textarea is the first thing in the box; there is no header row.
      expect(container.querySelector('.compose-top')).toBeNull();
    });

    it('shares approval copy and opens Settings without adding an inline editor', () => {
      store.config.value = configFixture;
      const { props } = renderComposer();
      const approvals = document.querySelector('.compose-approvals') as HTMLButtonElement;
      const shared = describeApprovals(configFixture);
      expect(approvals).not.toBeNull();
      expect(shared).not.toBeNull();
      expect(approvals.tagName).toBe('BUTTON');
      expect(approvals.type).toBe('button');
      expect(approvals.textContent).toBe(`Approvals: ${shared!.label}`);
      expect(approvals.getAttribute('title')).toBe(shared!.title);
      expect(approvals.getAttribute('title')).toContain('1 allowlisted command');
      expect(approvals.querySelector('input, select')).toBeNull();

      fireEvent.click(approvals);
      expect(props.onOpenSettings).toHaveBeenCalledTimes(1);
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
        expect(pill.querySelector('.context-pill-text')?.textContent).toBe('25.0%');
        expect(pill.textContent).toContain('Usage');
        expect(pill.getAttribute('title')).toContain('32,000 of 128,000 context tokens');
        expect(pill.getAttribute('aria-label')).toContain('25.0% full');
      });

      it('invokes exactly the caller-owned Usage action with the reported reading intact', () => {
        store.config.value = configFixture;
        withUsage(32_000);
        const onOpenUsage = vi.fn();
        renderComposer({ onOpenUsage });
        expect(workbench.contextOpen.value).toBe(false);

        fireEvent.click(
          screen.getByRole('button', { name: /Usage: context window 25\.0% full.*Open session usage/ }),
        );
        expect(onOpenUsage).toHaveBeenCalledExactlyOnceWith();
        // The workspace callback decides whether and how to open its drawer.
        expect(workbench.contextOpen.value).toBe(false);
      });

      it('keeps Usage as read-only telemetry without a destination, including on a narrow control plane', () => {
        store.config.value = configFixture;
        withUsage(32_000);
        store.narrowViewport.value = true;
        store.mobileSheet.value = 'sessions';
        const { container } = renderComposer();

        const pill = container.querySelector('.context-pill') as HTMLElement;
        expect(pill.tagName).toBe('SPAN');
        expect(pill.getAttribute('aria-label')).toBe(
          'Usage: context window 25.0% full, 32,000 of 128,000 tokens. Open session usage',
        );
        expect(pill.getAttribute('title')).toContain('32,000 of 128,000 context tokens used (25.0%)');
        expect(pill.querySelector('.context-pill-text')?.textContent).toBe('25.0%');
        expect(screen.queryByRole('button', { name: /Usage: context window/ })).toBeNull();

        fireEvent.click(pill);
        expect(workbench.contextOpen.value).toBe(false);
        expect(store.mobileSheet.value).toBe('sessions');
      });

      it('renders nothing before the first call, even with a draft, rather than estimating 0%', () => {
        store.config.value = configFixture;
        withUsage(null);
        const { container, textarea } = renderComposer();
        fireEvent.input(textarea, { target: { value: 'This unsubmitted draft is not provider usage.' } });
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
        expect(container.querySelector('.context-pill-text')?.textContent).toBe('25.0%');
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
      expect(hint.textContent?.replace(/\s+/g, ' ').trim()).toBe('Ctrl Enter to send·@ reference files');
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

  describe('drafts', () => {
    beforeEach(() => {
      store.currentSessionId.value = null;
      store.draftSession.value = null;
      store.draftToSession.value = null;
    });
    afterEach(() => {
      store.currentSessionId.value = null;
      store.draftSession.value = null;
      store.draftToSession.value = null;
    });

    it('carries the typed message across when its draft becomes a session', () => {
      store.draftSession.value = { id: 11, projectId: null, model: null };
      const { rerender, props, textarea } = renderComposer();
      fireEvent.input(textarea, { target: { value: 'half-written message' } });
      expect(textarea.value).toBe('half-written message');

      // The send (or an attachment that had to be uploaded) created the session:
      // same conversation, new scope key, so the box must follow it rather than
      // come back empty with the reader's text stranded in a record nothing draws.
      store.draftSession.value = null;
      store.currentSessionId.value = 's_new';
      store.draftToSession.value = { draftId: 11, sessionId: 's_new' };
      rerender(<Composer {...props} />);

      expect(textarea.value).toBe('half-written message');
    });

    it('gives a brand-new draft an empty box', () => {
      store.draftSession.value = { id: 21, projectId: null, model: null };
      const { rerender, props, textarea } = renderComposer();
      fireEvent.input(textarea, { target: { value: 'something else entirely' } });

      // A second conversation: nothing of the first one's text may follow.
      store.draftSession.value = { id: 22, projectId: null, model: null };
      rerender(<Composer {...props} />);

      expect(textarea.value).toBe('');
    });
  });

  describe('attachments', () => {
    // An upload lands under the session's own directory
    // (`POST /api/workspace/upload?session=`), so these are a session's composer.
    // An unsent draft creates its session on the first upload instead - see
    // `state/draftSession.test.ts`.
    beforeEach(() => {
      store.currentSessionId.value = 's_1';
    });
    afterEach(() => {
      store.currentSessionId.value = null;
    });

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

    it('always cancels disabled file drag/drop without attaching or showing the overlay', () => {
      const { form } = renderComposer({ disabledReason: 'Composer unavailable' });
      const dataTransfer = { files: [binaryFile('blocked.bin')], types: ['Files'], dropEffect: 'copy' };
      expect(fireEvent.dragEnter(form, { dataTransfer })).toBe(false);
      expect(fireEvent.dragOver(form, { dataTransfer })).toBe(false);
      expect(fireEvent.drop(form, { dataTransfer })).toBe(false);
      expect(dataTransfer.dropEffect).toBe('none');
      expect(screen.queryByText('blocked.bin')).toBeNull();
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

  describe('scoped pending work', () => {
    it('parks attachments as well as drafts for each session', async () => {
      store.currentSessionId.value = 's_a';
      const { textarea, fileInput } = renderComposer({
        uploadFile: vi.fn(async () => ({ path: '.karucode/attachments/s_a/file.bin' })),
      });
      fireEvent.change(fileInput, { target: { files: [binaryFile('file.bin')] } });
      fireEvent.input(textarea, { target: { value: 'for A' } });
      await screen.findByText(/Uploaded · 3 B/);

      await act(async () => {
        store.currentSessionId.value = 's_b';
      });
      expect(textarea.value).toBe('');
      expect(screen.queryByText('file.bin')).toBeNull();
      fireEvent.input(textarea, { target: { value: 'for B' } });

      await act(async () => {
        store.currentSessionId.value = 's_a';
      });
      expect(textarea.value).toBe('for A');
      expect(screen.getByText('file.bin')).not.toBeNull();
      await act(async () => {
        store.currentSessionId.value = 's_b';
      });
      expect(textarea.value).toBe('for B');
    });

    it('captures upload scope before awaiting and a late result never mutates the next project', async () => {
      store.currentProjectId.value = 'p_a';
      store.currentSessionId.value = 's_a';
      let finish: ((result: { path: string }) => void) | undefined;
      const uploadFile = vi.fn(
        (_file: File, _context: { session: string | null; project: string | null }) =>
          new Promise<{ path: string }>((resolve) => {
            finish = resolve;
          }),
      );
      const { textarea, fileInput } = renderComposer({ uploadFile });
      fireEvent.change(fileInput, { target: { files: [binaryFile('late.bin')] } });
      await waitFor(() => expect(uploadFile).toHaveBeenCalledTimes(1));
      expect(uploadFile.mock.calls[0]?.[1]).toEqual({ session: 's_a', project: 'p_a' });

      await act(async () => {
        store.currentProjectId.value = 'p_b';
        store.currentSessionId.value = 's_b';
      });
      fireEvent.input(textarea, { target: { value: 'draft B' } });
      await act(async () => {
        finish?.({ path: '.karucode/attachments/s_a/late.bin' });
        await Promise.resolve();
      });
      expect(textarea.value).toBe('draft B');
      expect(screen.queryByText('late.bin')).toBeNull();

      await act(async () => {
        store.currentProjectId.value = 'p_a';
        store.currentSessionId.value = 's_a';
      });
      expect(screen.getByText('late.bin')).not.toBeNull();
      expect(screen.getByText(/Uploaded · 3 B/)).not.toBeNull();
    });

    it('does not let a late send clear or busy the next session', async () => {
      store.currentSessionId.value = 's_a';
      let finish: ((accepted: boolean) => void) | undefined;
      const onSend = vi.fn(() => new Promise<boolean>((resolve) => { finish = resolve; }));
      const { props, textarea } = renderComposer({ onSend });
      fireEvent.input(textarea, { target: { value: 'send A' } });
      fireEvent.click(screen.getByRole('button', { name: /Send message/ }));
      expect(props.onSend).toHaveBeenCalledWith('send A');

      await act(async () => {
        store.currentSessionId.value = 's_b';
      });
      fireEvent.input(textarea, { target: { value: 'keep draft B' } });
      await act(async () => {
        finish?.(true);
        await Promise.resolve();
      });
      expect(textarea.value).toBe('keep draft B');
      expect((screen.getByRole('button', { name: /Send message/ }) as HTMLButtonElement).disabled).toBe(false);

      await act(async () => {
        store.currentSessionId.value = 's_a';
      });
      expect(textarea.value).toBe('');
      await act(async () => {
        store.currentSessionId.value = 's_b';
      });
      expect(textarea.value).toBe('keep draft B');
    });

    it('preserves text edited in the same scope while its send is pending', async () => {
      let finish: ((accepted: boolean) => void) | undefined;
      const { textarea } = renderComposer({
        onSend: vi.fn(() => new Promise<boolean>((resolve) => { finish = resolve; })),
      });
      fireEvent.input(textarea, { target: { value: 'submitted' } });
      fireEvent.click(screen.getByRole('button', { name: /Send message/ }));
      fireEvent.input(textarea, { target: { value: 'new work while pending' } });
      await act(async () => {
        finish?.(true);
        await Promise.resolve();
      });
      expect(textarea.value).toBe('new work while pending');
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
    it('ignores an old project response that arrives after the new project index', async () => {
      store.currentProjectId.value = 'p_one';
      const pending: Array<(tree: WorkspaceTree) => void> = [];
      const loadMentionTree = vi.fn(() => new Promise<WorkspaceTree>((resolve) => {
        pending.push(resolve);
      }));
      const { textarea } = renderComposer({ loadMentionTree });
      fireEvent.input(textarea, { target: { value: '@' } });
      await waitFor(() => expect(pending).toHaveLength(1));

      await act(async () => {
        store.currentProjectId.value = 'p_two';
      });
      fireEvent.input(textarea, { target: { value: '@' } });
      await waitFor(() => expect(pending).toHaveLength(2));
      await act(async () => {
        pending[1]?.({
          path: '.',
          entries: [{ name: 'two.ts', path: 'two.ts', kind: 'file', size: 1 }],
        });
      });
      expect(await screen.findByRole('option', { name: /two\.ts/ })).not.toBeNull();

      await act(async () => {
        pending[0]?.({
          path: '.',
          entries: [{ name: 'one.ts', path: 'one.ts', kind: 'file', size: 1 }],
        });
      });
      expect(screen.queryByRole('option', { name: /one\.ts/ })).toBeNull();
      expect(screen.getByRole('option', { name: /two\.ts/ })).not.toBeNull();
    });

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
      // Mention state is scoped with the draft, so open the new workspace's menu.
      fireEvent.input(textarea, { target: { value: '@main' } });

      await waitFor(() => expect(loadMentionTree).toHaveBeenCalledTimes(2));
    });
  });

  describe('steering a run in flight', () => {
    /** The composer as it looks mid-run, with a queue handler wired up. */
    function renderRunning(overrides: Partial<ComposerProps> = {}) {
      return renderComposer({
        running: true,
        status: { kind: 'streaming', label: 'Streaming' },
        onQueue: vi.fn(async () => true),
        ...overrides,
      });
    }

    it('offers Send only when idle and Steer when a run is working', () => {
      const idle = renderComposer();
      expect(screen.queryByRole('button', { name: /Steer/ })).toBeNull();
      idle.unmount();

      renderRunning();
      // The box queues rather than starting a second run, which is what the API
      // refuses: there is no Send button while the agent works.
      expect(screen.queryByRole('button', { name: /Send message/ })).toBeNull();
      expect(screen.getByRole('button', { name: /steer the running turn/ })).not.toBeNull();
    });

    it('separates configuration/Stop from send mode/send without dropping controls', async () => {
      const { container, textarea } = renderRunning({
        queued: [
          {
            id: 'still-visible',
            kind: 'steer',
            content: 'already queued',
            draft: 'already queued',
            accepted_at: 1,
          },
        ],
        onRecall: vi.fn(async () => true),
      });
      const bar = container.querySelector('.compose-bottom.is-running') as HTMLElement;
      const configuration = bar.querySelector('.compose-configuration') as HTMLElement;
      const actions = bar.querySelector('.compose-run-actions') as HTMLElement;
      const stop = screen.getByRole('button', { name: /Stop run/ });
      const send = screen.getByRole('button', { name: /steer the running turn/i });

      expect(bar).not.toBeNull();
      expect(configuration).not.toBeNull();
      expect(actions).not.toBeNull();
      expect(configuration.contains(screen.getByRole('button', { name: 'Attach files' }))).toBe(true);
      expect(
        within(configuration).getByRole('group', {
          name: 'Model and reasoning pins for subsequent runs in this session',
        }),
      ).not.toBeNull();
      // They are sibling grid areas on a phone, not nested controls that can hide
      // Stop when the second row wraps or the mode toggle appears.
      expect(stop.parentElement).toBe(bar);
      expect(configuration.contains(stop)).toBe(false);
      expect(actions.contains(stop)).toBe(false);
      expect(actions.contains(send)).toBe(true);
      expect(configuration.contains(send)).toBe(false);

      fireEvent.input(textarea, { target: { value: 'choose the next queue explicitly' } });
      await waitFor(() => expect(actions.contains(screen.getByRole('radiogroup'))).toBe(true));
      expect(screen.getByRole('button', { name: /Stop run/ })).toBe(stop);
      expect(screen.getByRole('region', { name: 'Queued messages' })).not.toBeNull();
    });

    it('queues the draft as a steer on Ctrl+Enter', async () => {
      const { props, textarea } = renderRunning();
      fireEvent.input(textarea, { target: { value: 'also check the fixtures' } });
      fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true });

      await waitFor(() => expect(props.onQueue).toHaveBeenCalledTimes(1));
      const [mode, message] = (props.onQueue as unknown as { mock: { calls: [string, { content: string; draft: string; attachments: unknown[] }][] } })
        .mock.calls[0]!;
      expect(mode).toBe('steer');
      expect(message.content).toBe('also check the fixtures');
      expect(message.draft).toBe('also check the fixtures');
      expect(message.attachments).toEqual([]);
      // The box empties only once the queue accepted it.
      await waitFor(() => expect(textarea.value).toBe(''));
    });

    it('switches to a follow-up and sends it as one', async () => {
      const { props, textarea } = renderRunning();
      fireEvent.input(textarea, { target: { value: 'then run the tests' } });

      const followUp = screen.getByRole('radio', { name: 'Follow-up' });
      fireEvent.click(followUp);
      expect(followUp.getAttribute('aria-checked')).toBe('true');

      fireEvent.click(screen.getByRole('button', { name: /queue this follow-up/i }));
      await waitFor(() => expect(props.onQueue).toHaveBeenCalledTimes(1));
      expect(
        (props.onQueue as unknown as { mock: { calls: [string][] } }).mock.calls[0]?.[0],
      ).toBe('follow_up');
    });

    it('makes the checked queue option the group’s only tab stop', async () => {
      const { textarea } = renderRunning();
      fireEvent.input(textarea, { target: { value: 'check the fixtures' } });
      await waitFor(() =>
        expect(screen.getByRole('radiogroup', { name: 'What this message does' })).not.toBeNull(),
      );

      const steer = screen.getByRole('radio', { name: 'Steer' });
      const followUp = screen.getByRole('radio', { name: 'Follow-up' });
      // One tab stop, like native radios: the group is entered once.
      expect(steer.getAttribute('tabindex')).toBe('0');
      expect(followUp.getAttribute('tabindex')).toBe('-1');

      fireEvent.click(followUp);
      expect(followUp.getAttribute('aria-checked')).toBe('true');
      expect(followUp.getAttribute('tabindex')).toBe('0');
      expect(steer.getAttribute('tabindex')).toBe('-1');
    });

    it('moves and selects with the arrow keys, wrapping at the ends', async () => {
      const { props, textarea } = renderRunning();
      fireEvent.input(textarea, { target: { value: 'then run the tests' } });
      await waitFor(() =>
        expect(screen.getByRole('radiogroup', { name: 'What this message does' })).not.toBeNull(),
      );

      const steer = screen.getByRole('radio', { name: 'Steer' });
      const followUp = screen.getByRole('radio', { name: 'Follow-up' });

      steer.focus();
      fireEvent.keyDown(steer, { key: 'ArrowRight' });
      expect(followUp.getAttribute('aria-checked')).toBe('true');
      // The selection and the focus move together, which is what makes it a radio
      // group rather than two independent buttons.
      expect(document.activeElement).toBe(followUp);

      // Right from the last option wraps to the first.
      fireEvent.keyDown(followUp, { key: 'ArrowRight' });
      expect(steer.getAttribute('aria-checked')).toBe('true');
      expect(document.activeElement).toBe(steer);

      // Left from the first wraps to the last.
      fireEvent.keyDown(steer, { key: 'ArrowLeft' });
      expect(followUp.getAttribute('aria-checked')).toBe('true');
      expect(document.activeElement).toBe(followUp);

      // Home and End are the ends of the group.
      fireEvent.keyDown(followUp, { key: 'Home' });
      expect(steer.getAttribute('aria-checked')).toBe('true');
      expect(document.activeElement).toBe(steer);
      fireEvent.keyDown(steer, { key: 'End' });
      expect(followUp.getAttribute('aria-checked')).toBe('true');
      expect(document.activeElement).toBe(followUp);

      // Down/Up are the same axis as Right/Left.
      fireEvent.keyDown(followUp, { key: 'ArrowUp' });
      expect(steer.getAttribute('aria-checked')).toBe('true');
      fireEvent.keyDown(steer, { key: 'ArrowDown' });
      expect(followUp.getAttribute('aria-checked')).toBe('true');

      // And the keyboard choice is the one that is sent.
      fireEvent.click(screen.getByRole('button', { name: /queue this follow-up/i }));
      await waitFor(() => expect(props.onQueue).toHaveBeenCalledTimes(1));
      expect(
        (props.onQueue as unknown as { mock: { calls: [string][] } }).mock.calls[0]?.[0],
      ).toBe('follow_up');
    });

    it('leaves the queue mode alone for keys it does not own', async () => {
      const { textarea } = renderRunning();
      fireEvent.input(textarea, { target: { value: 'x' } });
      await waitFor(() =>
        expect(screen.getByRole('radiogroup', { name: 'What this message does' })).not.toBeNull(),
      );
      const steer = screen.getByRole('radio', { name: 'Steer' });
      fireEvent.keyDown(steer, { key: 'Tab' });
      fireEvent.keyDown(steer, { key: 'a' });
      expect(steer.getAttribute('aria-checked')).toBe('true');
      expect(screen.getByRole('radio', { name: 'Follow-up' }).getAttribute('aria-checked')).toBe('false');
    });

    it('keeps the draft when the queue refuses the message', async () => {
      const onQueue = vi.fn(async () => false);
      const { textarea } = renderRunning({ onQueue });
      fireEvent.input(textarea, { target: { value: 'keep me' } });
      fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true });
      await waitFor(() => expect(onQueue).toHaveBeenCalled());
      // A 409 means the run just ended: the text must still be there to send.
      expect(textarea.value).toBe('keep me');
    });

    it('queues the attachment chips alongside the rendered content', async () => {
      const { props, textarea, fileInput } = renderRunning();
      fireEvent.change(fileInput, { target: { files: [textFile('notes.md', '# Note\n')] } });
      await waitFor(() => expect(screen.getByText(/Inlined · 7 B/)).not.toBeNull());

      fireEvent.input(textarea, { target: { value: 'summarise' } });
      fireEvent.click(screen.getByRole('button', { name: /steer the running turn/i }));
      await waitFor(() => expect(props.onQueue).toHaveBeenCalledTimes(1));
      const message = (props.onQueue as unknown as { mock: { calls: [string, { content: string; draft: string; attachments: { content?: string }[] }][] } })
        .mock.calls[0]![1];
      // The model gets the rendered body; the chip keeps its source so a recall
      // can rebuild it.
      expect(message.content).toContain('### notes.md (inlined, 7 B)');
      expect(message.draft).toBe('summarise');
      expect(message.attachments).toEqual([
        { name: 'notes.md', path: 'notes.md', mode: 'inline', size: 7, content: '# Note\n' },
      ]);
    });

    it('hides the mode toggle while the box is empty', async () => {
      const { textarea } = renderRunning();
      expect(screen.queryByRole('radiogroup', { name: 'What this message does' })).toBeNull();
      fireEvent.input(textarea, { target: { value: 'x' } });
      await waitFor(() =>
        expect(screen.getByRole('radiogroup', { name: 'What this message does' })).not.toBeNull(),
      );
    });

    it('says what Ctrl+Enter will do, and still stops on Escape', () => {
      const { props, container, textarea } = renderRunning();
      const hint = container.querySelector('.composer-hint') as HTMLElement;
      expect(hint.textContent?.replace(/\s+/g, ' ').trim()).toBe('Ctrl Enter to steer·Esc to stop');
      fireEvent.keyDown(textarea, { key: 'Escape' });
      expect(props.onStop).toHaveBeenCalledTimes(1);
    });

    it('stops accepting queue input while a stop is in flight, and says so', async () => {
      const { props, container, textarea } = renderRunning({ stopping: true });
      fireEvent.input(textarea, { target: { value: 'wait for the stop' } });

      // Queueing during a stop is what the server refuses with a 409: a steer has
      // no run to fold into and a follow-up has no turn to wait for yet.
      const queue = screen.getByRole('button', { name: /steer the running turn/i }) as HTMLButtonElement;
      expect(queue.disabled).toBe(true);
      fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true });
      await act(async () => {
        await Promise.resolve();
      });
      expect(props.onQueue).not.toHaveBeenCalled();
      // The stop is a pause, not a discard: the draft is still there to send once
      // the run has actually ended.
      expect(textarea.value).toBe('wait for the stop');

      // Assistive tech has to hear the wait: the button only changes its label and
      // disables itself, which is silent on its own.
      const stop = screen.getByRole('button', { name: /Stopping/ }) as HTMLButtonElement;
      expect(stop.disabled).toBe(true);
      expect(stop.getAttribute('aria-busy')).toBe('true');
      await waitFor(() => {
        const live = container.querySelector('.sr-only[aria-live]') as HTMLElement;
        expect(live.textContent).toContain('Stopping');
      });
    });
  });

  describe('queued messages', () => {
    const queuedSteer = {
      id: 'steer_1',
      kind: 'steer' as const,
      content: 'also check the fixtures',
      draft: 'also check the fixtures',
      accepted_at: 1_710_000_000_000,
    };

    function renderWithQueue(overrides: Partial<ComposerProps> = {}) {
      return renderComposer({
        running: true,
        status: { kind: 'streaming', label: 'Streaming' },
        onQueue: vi.fn(async () => true),
        queued: [queuedSteer],
        onRecall: vi.fn(async () => true),
        ...overrides,
      });
    }

    it('lists what is waiting, with its kind and attachments', () => {
      const { container } = renderWithQueue({
        queued: [
          queuedSteer,
          {
            id: 'followup_1',
            kind: 'follow_up',
            content: 'then run the tests',
            draft: 'then run the tests',
            accepted_at: 1_710_000_001_000,
            attachments: [
              { name: 'report.pdf', path: 'a/report.pdf', mode: 'upload', size: 4096 },
            ],
          },
        ],
      });
      const panel = screen.getByRole('region', { name: 'Queued messages' });
      expect(panel.textContent).toContain('2 messages waiting');
      expect(within(panel).getByText('also check the fixtures')).not.toBeNull();
      expect(within(panel).getByText('then run the tests')).not.toBeNull();
      // The chip is how the user recognises which file went with it.
      expect(within(panel).getByText('report.pdf')).not.toBeNull();
      // A follow-up waits its turn, so it is drawn differently from a steer.
      const rows = container.querySelectorAll('.queued-row');
      expect(rows[0]?.getAttribute('data-kind')).toBe('steer');
      expect(rows[1]?.getAttribute('data-kind')).toBe('follow_up');
    });

    it('renders nothing when the queue is empty', () => {
      renderWithQueue({ queued: [] });
      expect(screen.queryByRole('region', { name: 'Queued messages' })).toBeNull();
    });

    it('recalls a queued message by id', async () => {
      const onRecall = vi.fn(async () => true);
      renderWithQueue({ onRecall });
      fireEvent.click(screen.getByRole('button', { name: /Recall steer: also check the fixtures/ }));
      await waitFor(() => expect(onRecall).toHaveBeenCalledWith('steer_1'));
    });

    it('disables the button while a recall is in flight', () => {
      renderWithQueue({ recalling: { steer_1: true } });
      const button = screen.getByRole('button', { name: /Recall steer/ }) as HTMLButtonElement;
      expect(button.disabled).toBe(true);
      expect(button.textContent).toBe('Recalling…');
    });

    it('puts a recalled draft and its chips back in the composer', async () => {
      const { textarea } = renderWithQueue({ queued: [] });
      // What `actions.recallQueuedMessage` does with the server's answer.
      await act(async () => {
        store.composerHandoff.value = {
          text: 'summarise this',
          attachments: [
            { name: 'notes.md', path: 'notes.md', mode: 'inline', size: 7, content: '# Note\n' },
          ],
          token: 1,
        };
      });

      await waitFor(() => expect(textarea.value).toBe('summarise this'));
      expect(store.composerHandoff.value).toBeNull();
      // The chip is back, ready to send, so the next send is the same request.
      expect(screen.getByText('notes.md')).not.toBeNull();
      expect(screen.getByText(/Inlined · 7 B/)).not.toBeNull();
    });

    it('appends a recalled draft to what the user has typed since', async () => {
      const { textarea } = renderWithQueue({ queued: [] });
      fireEvent.input(textarea, { target: { value: 'new thought' } });

      await act(async () => {
        store.composerHandoff.value = { text: 'old thought', attachments: [], token: 2 };
      });

      // A recall must never silently replace a draft written after it was queued.
      await waitFor(() => expect(textarea.value).toBe('new thought\n\nold thought'));
    });
  });

  describe('plugin-contributed controls', () => {
    function withUi(ui: unknown): void {
      store.config.value = { ...configFixture, plugin_ui: ui as never };
    }

    it('draws a plugin button in its action row and sends exactly one action per click', async () => {
      withUi({
        contributions: [
          {
            plugin: 'kc-focus',
            version: '1.0.0',
            kind: 'button',
            id: 'focus',
            hosted_id: 'plugin__kc-focus__focus',
            slot: 'composer.toolbar',
            label: 'Focus mode',
            title: 'Focus mode',
            action: 'focus',
          },
        ],
      });
      vi.stubGlobal(
        'fetch',
        vi.fn(async () =>
          new Response(
            JSON.stringify({ ok: true, message: 'Focused', refresh: [], open_url: null }),
            { status: 200, headers: { 'Content-Type': 'application/json' } },
          ),
        ),
      );
      try {
        const { container } = renderComposer();
        const row = container.querySelector('.compose-bar-left');
        if (!row) throw new Error('the composer action row did not render');
        expect(within(row as HTMLElement).getByRole('button', { name: 'Focus mode' })).toBeTruthy();

        fireEvent.click(screen.getByRole('button', { name: 'Focus mode' }));
        await waitFor(() =>
          expect(store.toasts.value.map((toast) => toast.message)).toContain('Focused'),
        );
        // One click, one request: the control locks itself while it waits.
        expect((window.fetch as unknown as { mock: { calls: unknown[] } }).mock.calls).toHaveLength(1);
      } finally {
        vi.unstubAllGlobals();
      }
    });

    it('lets a plugin hide the attach button without touching the rest of the row', () => {
      withUi({ hidden: [{ target: 'composer.attach', plugins: ['kc-focus'] }] });
      const { container } = renderComposer();
      expect(container.querySelector('.compose-plus')).toBeNull();
      expect(screen.getByLabelText('Message karucode')).toBeTruthy();
      // Model controls are not hideable, so they stay.
      expect(container.querySelector('.composer-session-pins')).not.toBeNull();
    });

    it('keeps the attach button when nothing is hidden', () => {
      withUi(undefined);
      const { container } = renderComposer();
      expect(container.querySelector('.compose-plus')).not.toBeNull();
    });
  });
});