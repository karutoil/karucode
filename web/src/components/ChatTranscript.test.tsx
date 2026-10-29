import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { ChatTranscript } from './ChatTranscript';
import type { ApprovalView, ChatItem, CommandItem, MessageItem, ToolItem } from '../state/chat';
import { approvalFixture, NOW } from '../test/fixtures';

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

function renderTranscript(items: ChatItem[], onApprovalDecision = vi.fn()) {
  return render(
    <ChatTranscript items={items} resolvingApprovals={{}} onApprovalDecision={onApprovalDecision} />,
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
