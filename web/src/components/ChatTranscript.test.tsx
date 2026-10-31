import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/preact';
import { ChatTranscript } from './ChatTranscript';
import type { ApprovalView, ChatItem, CommandItem, MessageItem, ToolItem } from '../state/chat';
import { approvalFixture, NOW } from '../test/fixtures';
import { resetTheme, updateTheme } from '../theme';
import * as store from '../state/store';

function message(overrides: Partial<MessageItem> = {}): MessageItem {
  return {
    kind: 'message',
    id: 'm1',
    at: NOW,
    role: 'assistant',
    content: 'Mapped the registry.',
    reasoning: '',
    streaming: false,
    runId: 'r_x',
    agentId: null,
    ...overrides,
  };
}

function user(content = 'Build a plugin lifecycle.', overrides: Partial<MessageItem> = {}): MessageItem {
  return message({ id: 'u1', role: 'user', content, ...overrides });
}

function tool(overrides: Partial<ToolItem> = {}): ToolItem {
  return {
    kind: 'tool',
    id: 't1',
    at: NOW,
    callId: 'call_1',
    name: 'read_file',
    args: { path: 'src/main.rs' },
    status: 'ok',
    output: 'fn main() {}\n',
    truncated: false,
    durationMs: 12,
    runId: 'r_x',
    ...overrides,
  };
}

function command(overrides: Partial<CommandItem> = {}): CommandItem {
  return {
    kind: 'command',
    id: 'c1',
    at: NOW,
    callId: 'call_2',
    command: 'cargo test',
    cwd: '.',
    stdout: 'ok\n',
    stderr: '',
    output: '',
    exitCode: 0,
    status: 'ok',
    truncated: false,
    durationMs: 900,
    runId: 'r_x',
    ...overrides,
  };
}

function approvalView(overrides: Partial<ApprovalView> = {}): ApprovalView {
  return {
    id: approvalFixture.id,
    kind: approvalFixture.kind,
    title: approvalFixture.title,
    detail: approvalFixture.detail,
    payload: approvalFixture.payload,
    session_id: approvalFixture.session_id,
    run_id: approvalFixture.run_id,
    agent_id: null,
    status: 'pending',
    decision: null,
    created_at: NOW,
    ...overrides,
  };
}

function renderTranscript(
  items: ChatItem[],
  onApprovalDecision = vi.fn(),
  onRewind?: (target: { messageId: number; preview: string; isAssistant: boolean }) => void,
) {
  return render(
    <ChatTranscript
      items={items}
      resolvingApprovals={{}}
      onApprovalDecision={onApprovalDecision}
      onRewind={onRewind}
    />,
  );
}

describe('ChatTranscript', () => {
  it('renders one assistant row per turn, with the cards inside its body', () => {
    renderTranscript([user(), message(), tool(), message({ id: 'm2', content: 'Changes are ready.' })]);

    const turns = document.querySelectorAll('.msg-assistant');
    expect(turns).toHaveLength(1);
    expect(turns[0]?.querySelectorAll('.msg-avatar')).toHaveLength(1);
    // The tool card is part of the same body column as the prose.
    expect(turns[0]?.querySelector('.msg-body .tool-card')).not.toBeNull();
    expect(screen.getByText('Mapped the registry.')).not.toBeNull();
    expect(screen.getByText('Changes are ready.')).not.toBeNull();
  });

  it('starts a new turn after every user message', () => {
    renderTranscript([
      user('first'),
      message({ id: 'a1' }),
      user('second', { id: 'u2' }),
      message({ id: 'a2' }),
    ]);

    expect(document.querySelectorAll('.msg-assistant')).toHaveLength(2);
    expect(document.querySelectorAll('.msg-user')).toHaveLength(2);
    // The mock's row rhythm: avatar + author for each speaker.
    expect(screen.getAllByText('karucode')).toHaveLength(2);
    expect(screen.getAllByText('You')).toHaveLength(2);
  });

  it('labels the rows for screen readers', () => {
    renderTranscript([user(), message()]);
    expect(screen.getByLabelText('Your message')).not.toBeNull();
    expect(screen.getByLabelText('karucode response')).not.toBeNull();
  });

  it('shows a date marker before the first message of each day', () => {
    const today = Date.now();
    const yesterday = today - 86_400_000;
    renderTranscript([
      message({ id: 'a1', at: yesterday, content: 'old' }),
      user('new', { at: today }),
      message({ id: 'a2', at: today, content: 'new answer' }),
    ]);

    const markers = [...document.querySelectorAll('.date-marker')].map((node) => node.textContent ?? '');
    expect(markers).toHaveLength(2);
    expect(markers.some((text) => text.startsWith('Today ·'))).toBe(true);
    expect(markers.some((text) => text.startsWith('Yesterday ·'))).toBe(true);
  });

  it('marks a completed turn with the mock tag', () => {
    renderTranscript([user(), message()]);
    expect(document.querySelector('.msg-assistant .tag')?.textContent).toContain('Completed');
  });

  it('marks a live turn as Working', () => {
    renderTranscript([user(), message({ streaming: true, content: 'Half an ans' })]);
    const tag = document.querySelector('.msg-assistant .tag');
    expect(tag?.textContent).toContain('Working');
    expect(tag?.className).toContain('tag-live');
    expect(document.querySelector('.stream-caret')).not.toBeNull();
  });

  describe('throughput badge', () => {
    it('appears beside the Working chip while the turn produces text', () => {
      renderTranscript([user(), message({ streaming: true, content: 'x'.repeat(8_000) })]);
      const badge = document.querySelector('.msg-assistant .tps-badge');
      expect(badge, 'the badge is mounted').not.toBeNull();
      // 8 000 characters at four chars per token, sampled on mount. Exact below
      // 10k, so the count reads as itself rather than a rounded "2k".
      expect(badge?.querySelector('.tps-badge-count')?.textContent).toBe('2000');
    });

    it('stays while the turn is running a tool call', () => {
      // The prose is finished but the turn is not: the count is still the only
      // evidence of what the model produced, so the badge stays.
      renderTranscript([
        user(),
        message({ content: 'Running the suite.' }),
        tool({ status: 'running' }),
      ]);
      expect(document.querySelector('.msg-assistant .tps-badge')).not.toBeNull();
      // …and the turn is not reported as finished while a call is in flight.
      expect(document.querySelector('.msg-assistant .tag')?.textContent).toContain('Working');
    });

    it('is gone once the turn has settled', () => {
      renderTranscript([user(), message(), tool()]);
      expect(document.querySelector('.tps-badge')).toBeNull();
    });

    it('is absent from the user row', () => {
      renderTranscript([user('hello')]);
      expect(document.querySelector('.msg-user .tps-badge')).toBeNull();
    });
  });

  it('reports a failed turn instead of claiming it completed', () => {
    renderTranscript([
      user(),
      message(),
      { kind: 'error', id: 'e1', at: NOW, message: 'provider exploded', errorKind: 'stream', runId: 'r_x' },
    ]);
    expect(document.querySelector('.msg-assistant .tag')?.textContent).toContain('Failed');
    expect(screen.getByRole('alert').textContent).toContain('provider exploded');
  });

  it('reports an interrupted turn', () => {
    renderTranscript([
      user(),
      message(),
      { kind: 'interrupted', id: 'i1', at: NOW, runId: 'r_x', message: 'Stopped by you.' },
    ]);
    expect(document.querySelector('.msg-assistant .tag')?.textContent).toContain('Interrupted');
    expect(screen.getByText(/Stopped by you\./)).not.toBeNull();
  });

  it('renders commands inside the turn and keeps running ones open', () => {
    renderTranscript([user(), command({ status: 'running', exitCode: null, durationMs: null })]);
    const card = document.querySelector('.msg-body .command-card');
    expect(card).not.toBeNull();
    expect(card?.getAttribute('data-open')).toBe('true');
  });

  it('keeps an approval card in the turn and reports the decision', () => {
    const onDecide = vi.fn();
    renderTranscript(
      [user(), message(), { kind: 'approval', id: 'ap', at: NOW, approval: approvalView() }],
      onDecide,
    );

    expect(document.querySelector('.msg-body .approval-card')).not.toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Allow once' }));
    expect(onDecide).toHaveBeenCalledWith(approvalFixture.id, 'allow_once');
  });

  it('renders an assistant tool call that arrives without prose', () => {
    renderTranscript([tool()]);
    expect(document.querySelectorAll('.msg-assistant')).toHaveLength(1);
    expect(document.querySelector('.msg-body .tool-card')).not.toBeNull();
  });

  it('renders notices with the right live-region role', () => {
    renderTranscript([
      user(),
      { kind: 'notice', id: 'n1', at: NOW, level: 'warn', message: 'Context is nearly full.' },
    ]);
    expect(screen.getByRole('status').textContent).toContain('Context is nearly full.');
  });

  it('shows an empty transcript without inventing rows', () => {
    renderTranscript([]);
    expect(document.querySelectorAll('.msg')).toHaveLength(0);
    expect(document.querySelector('.date-marker')).toBeNull();
  });
});

describe('ChatTranscript view options', () => {
  function assistantWithReasoning(overrides: Partial<MessageItem> = {}): MessageItem {
    return message({
      id: 'mr',
      content: '',
      reasoning: 'The config lives in settings.rs.',
      ...overrides,
    });
  }

  beforeEach(() => {
    localStorage.clear();
    resetTheme();
  });

  afterEach(() => {
    resetTheme();
  });

  function setOptions(patch: { toolStyle?: 'cards' | 'rows'; grouping?: 'none' | 'tools' | 'turns'; thinking?: 'collapsed' | 'preview' | 'expanded' }) {
    act(() => {
      updateTheme(patch);
    });
  }

  it('draws tool calls as cards by default', () => {
    renderTranscript([user(), tool()]);
    expect(document.querySelector('.msg-body .tool-card')).not.toBeNull();
    expect(document.querySelector('.tool-row')).toBeNull();
  });

  it('draws tool calls as compact rows when the theme asks for it', () => {
    setOptions({ toolStyle: 'rows' });
    renderTranscript([user(), tool()]);

    expect(document.querySelector('.tool-card')).toBeNull();
    expect(document.querySelector('.msg-body .tool-row')).not.toBeNull();
    // The row still exposes its output behind the same disclosure contract.
    const head = document.querySelector('.tool-row-head');
    expect(head?.getAttribute('aria-expanded')).toBe('false');
  });

  it('leaves consecutive calls alone with grouping off', () => {
    renderTranscript([user(), tool({ id: 't1' }), tool({ id: 't2', callId: 'c2' }), message()]);
    expect(document.querySelectorAll('.tool-group')).toHaveLength(0);
    expect(document.querySelectorAll('.tool-card')).toHaveLength(2);
  });

  it('folds a run of consecutive calls into one line', () => {
    setOptions({ grouping: 'tools' });
    renderTranscript([
      user(),
      tool({ id: 't1', name: 'read_file' }),
      tool({ id: 't2', callId: 'c2', name: 'grep' }),
      message({ id: 'a2' }),
    ]);

    const group = document.querySelector('.tool-group');
    expect(group).not.toBeNull();
    expect(group?.querySelector('.tool-group-summary')?.textContent).toContain('read_file');
    expect(group?.querySelector('.tool-group-count')?.textContent).toBe('2 calls');
    // Collapsed by default because nothing is running and nothing failed.
    expect(group?.querySelector('.tool-group-head')?.getAttribute('aria-expanded')).toBe('false');
  });

  it('marks prose between calls as narration, and the final answer as not', () => {
    renderTranscript([
      user(),
      message({ id: 'n1', content: 'Let me read the config.' }),
      tool(),
      message({ id: 'fin', content: 'The pool size is 16.' }),
    ]);

    const narrated = document.querySelectorAll('.narration');
    expect(narrated).toHaveLength(1);
    expect(narrated[0]?.textContent).toContain('Let me read the config.');
    expect(screen.getByText('The pool size is 16.').closest('.narration')).toBeNull();
  });

  it('folds a finished turn behind a summary of what it did', () => {
    setOptions({ grouping: 'turns' });
    renderTranscript([
      user(),
      message({ id: 'n1', content: 'Reading the config.' }),
      tool({ id: 't1' }),
      tool({ id: 't2', callId: 'c2', name: 'grep' }),
      message({ id: 'fin', content: 'The pool size is 16.' }),
    ]);

    const fold = document.querySelector('.process-fold');
    expect(fold).not.toBeNull();
    expect(fold?.getAttribute('aria-expanded')).toBe('false');
    expect(fold?.textContent).toContain('2 tool calls');
    // The answer survives the fold; the working steps do not.
    expect(screen.getByText('The pool size is 16.')).not.toBeNull();
    expect(screen.queryByText('Reading the config.')).toBeNull();
    expect(document.querySelector('.tool-group')).toBeNull();
    expect(document.querySelector('.tool-card')).toBeNull();
  });

  it('opens the folded turn from its summary line', () => {
    setOptions({ grouping: 'turns' });
    renderTranscript([
      user(),
      message({ id: 'n1', content: 'Reading the config.' }),
      tool(),
      message({ id: 'fin', content: 'The pool size is 16.' }),
    ]);

    fireEvent.click(document.querySelector('.process-fold') as Element);
    expect(screen.getByText('Reading the config.')).not.toBeNull();
    expect(document.querySelector('.tool-row, .tool-card')).not.toBeNull();
    expect(document.querySelector('.process-fold')?.getAttribute('aria-expanded')).toBe('true');
  });

  it('never folds a turn that is still working', () => {
    setOptions({ grouping: 'turns' });
    renderTranscript([
      user(),
      tool(),
      message({ id: 'fin', content: 'Working on it', streaming: true }),
    ]);
    expect(document.querySelector('.process-fold')).toBeNull();
    expect(document.querySelector('.tool-row, .tool-card')).not.toBeNull();
  });

  it('never folds a turn that is waiting on an approval', () => {
    setOptions({ grouping: 'turns' });
    renderTranscript([
      user(),
      tool(),
      message({ id: 'fin', content: 'I need permission.' }),
      { kind: 'approval', id: 'ap', at: NOW, approval: approvalView() },
    ]);
    expect(document.querySelector('.process-fold')).toBeNull();
    expect(screen.getByRole('button', { name: 'Allow once' })).not.toBeNull();
  });

  it('keeps a notice visible above the fold instead of hiding it', () => {
    setOptions({ grouping: 'turns' });
    renderTranscript([
      user(),
      { kind: 'notice', id: 'n1', at: NOW, level: 'warn', message: 'Context is nearly full.' },
      tool(),
      message({ id: 'fin', content: 'Done, with caveats.' }),
    ]);
    expect(document.querySelector('.process-fold')).not.toBeNull();
    expect(screen.getByRole('status').textContent).toContain('Context is nearly full.');
  });

  it('passes the thinking mode to the reasoning block', () => {
    setOptions({ thinking: 'preview' });
    renderTranscript([user(), assistantWithReasoning()]);

    const preview = document.querySelector('.thinking-preview');
    expect(document.querySelector('.thinking')?.getAttribute('data-mode')).toBe('preview');
    expect(preview?.textContent).toContain('The config lives in settings.rs.');
  });

  it('shows reasoning open when the theme asks for it', () => {
    setOptions({ thinking: 'expanded' });
    renderTranscript([user(), assistantWithReasoning()]);
    expect(document.querySelector('.thinking')?.hasAttribute('open')).toBe(true);
  });
});

describe('ChatTranscript rewind affordance', () => {
  it('offers a rewind on a persisted user message and names its id', () => {
    const onRewind = vi.fn();
    renderTranscript([user('Build a plugin lifecycle.', { id: 'msg:412' })], vi.fn(), onRewind);

    const button = screen.getByRole('button', { name: 'Rewind to this message' });
    fireEvent.click(button);
    expect(onRewind).toHaveBeenCalledWith({
      messageId: 412,
      preview: 'Build a plugin lifecycle.',
      isAssistant: false,
    });
  });

  it('offers a rewind on the answer, which means the request behind it', () => {
    const onRewind = vi.fn();
    renderTranscript(
      [user('go', { id: 'msg:1' }), message({ id: 'msg:2', content: 'Done.' })],
      vi.fn(),
      onRewind,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Rewind to the request that produced this answer' }));
    expect(onRewind).toHaveBeenCalledWith({
      messageId: 2,
      preview: 'Done.',
      isAssistant: true,
    });
  });

  it('offers nothing for an item the server cannot resolve', () => {
    const onRewind = vi.fn();
    // `local:` is an optimistic send and `stream:` is a live placeholder; neither
    // has a row the rewind endpoint could look up.
    renderTranscript([user('pending', { id: 'local:1' })], vi.fn(), onRewind);
    expect(screen.queryByRole('button', { name: /Rewind/ })).toBeNull();
  });

  it('hides the affordance while the agent is still writing', () => {
    store.chat.value = { ...store.chat.value, running: true };
    renderTranscript([user('go', { id: 'msg:1' })], vi.fn(), vi.fn());
    expect(screen.queryByRole('button', { name: /Rewind/ })).toBeNull();
  });

  it('offers nothing when the transcript is read-only', () => {
    renderTranscript([user('go', { id: 'msg:1' })]);
    expect(screen.queryByRole('button', { name: /Rewind/ })).toBeNull();
  });
});
