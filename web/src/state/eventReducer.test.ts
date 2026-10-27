import { describe, expect, it } from 'vitest';
import type { KarucodeEvent, SessionSummary } from '../api/types';
import { emptyChatState, seedChatState, type ChatState, type MessageItem } from './chat';
import { applyEventToChat, applyEventToSessions } from './eventReducer';
import { detailFixture, NOW, summaryFixture, secondSummaryFixture } from '../test/fixtures';

function state(): ChatState {
  return emptyChatState('s_ab12');
}

function envelope(seq: number, overrides: Partial<KarucodeEvent> = {}): KarucodeEvent {
  return {
    seq,
    ts_ms: NOW + seq,
    session_id: 's_ab12',
    run_id: 'r_x',
    ...overrides,
  } as KarucodeEvent;
}

function event<T extends KarucodeEvent['type']>(
  seq: number,
  type: T,
  fields: Record<string, unknown> = {},
): KarucodeEvent {
  return envelope(seq, { type, ...fields } as Partial<KarucodeEvent>);
}

function lastMessage(current: ChatState): MessageItem {
  const item = [...current.items].reverse().find((candidate) => candidate.kind === 'message');
  if (!item || item.kind !== 'message') throw new Error('no message item');
  return item;
}

describe('applyEventToChat', () => {
  it('starts a run and reuses the optimistic user message', () => {
    let current: ChatState = {
      ...state(),
      running: true,
      items: [
        {
          kind: 'message',
          id: 'local:1',
          at: NOW,
          role: 'user',
          content: 'Hello',
          reasoning: '',
          streaming: false,
          runId: null,
          agentId: null,
        },
      ],
    };

    current = applyEventToChat(current, event(1, 'run_started', { prompt: 'Hello' }));

    const users = current.items.filter((item) => item.kind === 'message' && item.role === 'user');
    expect(users).toHaveLength(1);
    expect(users[0]?.kind === 'message' ? users[0].runId : null).toBe('r_x');
    expect(current.running).toBe(true);
    expect(current.runId).toBe('r_x');
    expect(current.runStatus).toBe('running');
  });

  it('adds a user message from run_started when there is no optimistic one', () => {
    const current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Explain the repo' }));
    const user = current.items.find((item) => item.kind === 'message' && item.role === 'user');
    expect(user?.kind === 'message' ? user.content : '').toBe('Explain the repo');
  });

  it('streams text deltas into one assistant segment and closes it at tool calls', () => {
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(current, event(2, 'text_delta', { delta: 'Hello' }));
    current = applyEventToChat(current, event(3, 'text_delta', { delta: ' world' }));

    const first = lastMessage(current);
    expect(first.content).toBe('Hello world');
    expect(first.streaming).toBe(true);

    current = applyEventToChat(
      current,
      event(4, 'tool_call_started', { call_id: 'call_1', name: 'read_file', arguments: { path: 'a.rs' } }),
    );

    expect(lastMessage(current).streaming).toBe(false);
    const tool = current.items.find((item) => item.kind === 'tool');
    expect(tool?.kind === 'tool' ? tool.name : '').toBe('read_file');

    current = applyEventToChat(current, event(5, 'text_delta', { delta: 'After the tool.' }));
    const messages = current.items.filter(
      (item) => item.kind === 'message' && item.role === 'assistant',
    );
    expect(messages).toHaveLength(2);
    expect(lastMessage(current).content).toBe('After the tool.');
  });

  it('streams reasoning separately and keeps it on the same segment', () => {
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(current, event(2, 'reasoning_delta', { delta: 'First, inspect. ' }));
    current = applyEventToChat(current, event(3, 'reasoning_delta', { delta: 'Then edit.' }));
    current = applyEventToChat(current, event(4, 'text_delta', { delta: 'Done.' }));

    const message = lastMessage(current);
    expect(message.reasoning).toBe('First, inspect. Then edit.');
    expect(message.content).toBe('Done.');
  });

  it('finishes a tool call with output, status and duration', () => {
    let current = applyEventToChat(
      state(),
      event(1, 'tool_call_started', { call_id: 'call_1', name: 'read_file', arguments: { path: 'a.rs' } }),
    );
    current = applyEventToChat(
      current,
      event(2, 'tool_call_finished', {
        call_id: 'call_1',
        name: 'read_file',
        ok: true,
        output: 'fn main() {}',
        truncated: true,
        duration_ms: 42,
      }),
    );
    const tool = current.items.find((item) => item.kind === 'tool');
    expect(tool).toMatchObject({
      kind: 'tool',
      status: 'ok',
      output: 'fn main() {}',
      truncated: true,
      durationMs: 42,
    });
  });

  it('upgrades a run_command tool card into a streaming command card', () => {
    let current = applyEventToChat(
      state(),
      event(1, 'tool_call_started', {
        call_id: 'call_cmd',
        name: 'run_command',
        arguments: { command: 'cargo test', cwd: '.' },
      }),
    );
    current = applyEventToChat(
      current,
      event(2, 'command_started', { call_id: 'call_cmd', command: 'cargo test', cwd: '.' }),
    );
    current = applyEventToChat(
      current,
      event(3, 'command_output', { call_id: 'call_cmd', stream: 'stdout', chunk: 'running 3 tests\n' }),
    );
    current = applyEventToChat(
      current,
      event(4, 'command_output', { call_id: 'call_cmd', stream: 'stderr', chunk: 'warning: unused\n' }),
    );
    current = applyEventToChat(
      current,
      event(5, 'command_finished', { call_id: 'call_cmd', exit_code: 1, duration_ms: 1200, truncated: false }),
    );
    current = applyEventToChat(
      current,
      event(6, 'tool_call_finished', {
        call_id: 'call_cmd',
        name: 'run_command',
        ok: false,
        output: 'exit code: 1',
        truncated: false,
        duration_ms: 1200,
      }),
    );

    const commands = current.items.filter((item) => item.kind === 'command');
    expect(commands).toHaveLength(1);
    expect(commands[0]).toMatchObject({
      stdout: 'running 3 tests\n',
      stderr: 'warning: unused\n',
      exitCode: 1,
      status: 'error',
      durationMs: 1200,
    });
    expect(current.items.filter((item) => item.kind === 'tool')).toHaveLength(0);
  });

  it('adds and resolves approvals without duplicating them', () => {
    let current = applyEventToChat(
      state(),
      event(1, 'approval_requested', {
        approval: {
          id: 'ap_1',
          kind: 'shell_command',
          title: 'Run cargo test',
          detail: 'cargo test',
          payload: { command: 'cargo test' },
          session_id: 's_ab12',
          run_id: 'r_x',
        },
      }),
    );
    current = applyEventToChat(current, event(2, 'approval_requested', {
      approval: {
        id: 'ap_1',
        kind: 'shell_command',
        title: 'Run cargo test',
        detail: 'cargo test',
        payload: { command: 'cargo test' },
        session_id: 's_ab12',
        run_id: 'r_x',
      },
    }));
    expect(current.items.filter((item) => item.kind === 'approval')).toHaveLength(1);

    current = applyEventToChat(
      current,
      event(3, 'approval_resolved', { approval_id: 'ap_1', decision: 'allow_session' }),
    );
    const approval = current.items.find((item) => item.kind === 'approval');
    expect(approval?.kind === 'approval' ? approval.approval.status : null).toBe('resolved');
    expect(approval?.kind === 'approval' ? approval.approval.decision : null).toBe('allow_session');
  });

  it('records run errors, stops active work and closes the stream', () => {
    let current = applyEventToChat(
      state(),
      event(1, 'tool_call_started', { call_id: 'call_1', name: 'read_file', arguments: {} }),
    );
    current = applyEventToChat(current, event(2, 'text_delta', { delta: 'partial' }));
    // The text_delta above opens a new streaming segment after the tool card.
    current = applyEventToChat(
      current,
      event(3, 'run_finished', { status: 'failed', error: 'provider exploded' }),
    );

    expect(current.running).toBe(false);
    expect(current.runStatus).toBe('failed');
    expect(current.runError?.message).toBe('provider exploded');
    const tool = current.items.find((item) => item.kind === 'tool');
    expect(tool?.kind === 'tool' ? tool.status : null).toBe('stopped');
    const assistant = current.items.filter((item) => item.kind === 'message' && item.role === 'assistant');
    expect(assistant.every((item) => item.kind === 'message' && !item.streaming)).toBe(true);
  });

  it('adds an interrupted notice for interrupted runs and a notice for cancellation', () => {
    let current = applyEventToChat(state(), event(1, 'run_finished', { status: 'interrupted' }));
    expect(current.items.some((item) => item.kind === 'interrupted')).toBe(true);

    current = applyEventToChat(current, event(2, 'run_cancelled', { run_id: 'r_y' }));
    expect(current.items.some((item) => item.kind === 'notice')).toBe(true);
    // A duplicate run_finished(cancelled) notice must not be duplicated.
    current = applyEventToChat(current, event(3, 'run_finished', { run_id: 'r_y', status: 'cancelled' }));
    expect(current.items.filter((item) => item.kind === 'notice')).toHaveLength(1);
  });

  it('accumulates usage and ignores subagent deltas in the main transcript', () => {
    let current = applyEventToChat(state(), event(1, 'usage', { input_tokens: 10, output_tokens: 4 }));
    current = applyEventToChat(current, event(2, 'usage', { input_tokens: 5, output_tokens: 2 }));
    expect(current.usage).toEqual({ input_tokens: 15, output_tokens: 6 });

    const before = current;
    current = applyEventToChat(
      current,
      envelope(3, { agent_id: 'a_1', type: 'text_delta', delta: 'subagent output' } as Partial<KarucodeEvent>),
    );
    expect(current).toBe(before);
  });

  it('tracks subagent lifecycle events', () => {
    let current = applyEventToChat(
      state(),
      event(1, 'agent_spawned', {
        agent_id: 'a_1',
        name: 'reviewer',
        role: 'reviewer',
        task: 'Review the diff',
        model: 'gpt-4o-mini',
        parent_run_id: 'r_x',
      }),
    );
    current = applyEventToChat(current, event(2, 'agent_progress', { agent_id: 'a_1', iterations: 2, note: '' }));
    current = applyEventToChat(
      current,
      event(3, 'agent_finished', {
        agent_id: 'a_1',
        status: 'completed',
        summary: 'Looks good',
        error: null,
      }),
    );

    expect(current.agents).toHaveLength(1);
    expect(current.agents[0]).toMatchObject({
      id: 'a_1',
      iterations: 2,
      status: 'completed',
      summary: 'Looks good',
    });
    expect(current.agents[0]?.finished_at).not.toBeNull();
  });

  it('ignores events from another session and unknown event types', () => {
    const current = state();
    expect(applyEventToChat(current, envelope(1, { session_id: 's_other', type: 'text_delta', delta: 'x' } as Partial<KarucodeEvent>))).toBe(current);
    expect(applyEventToChat(current, envelope(2, { type: 'future_event', payload: 1 } as unknown as Partial<KarucodeEvent>))).toBe(current);
  });
});

describe('applyEventToSessions', () => {
  const sessions: SessionSummary[] = [summaryFixture, secondSummaryFixture];

  it('adds, renames and deletes sessions', () => {
    let list = applyEventToSessions(sessions, event(1, 'session_created', {
      session_id: 's_new',
      title: 'New session',
    }));
    expect(list).toHaveLength(3);
    expect(list.find((session) => session.id === 's_new')?.title).toBe('New session');

    list = applyEventToSessions(list, event(2, 'session_renamed', {
      session_id: 's_new',
      title: 'Renamed',
    }));
    expect(list.find((session) => session.id === 's_new')?.title).toBe('Renamed');

    list = applyEventToSessions(list, event(3, 'session_deleted', { session_id: 's_new' }));
    expect(list.find((session) => session.id === 's_new')).toBeUndefined();
  });

  it('marks runs running and finished', () => {
    let list = applyEventToSessions(sessions, event(1, 'run_started', { prompt: 'hi' }));
    expect(list.find((session) => session.id === 's_ab12')?.running).toBe(true);
    expect(list.find((session) => session.id === 's_ab12')?.message_count).toBe(13);

    list = applyEventToSessions(list, event(2, 'run_finished', { status: 'failed', error: 'x' }));
    const updated = list.find((session) => session.id === 's_ab12');
    expect(updated?.running).toBe(false);
    expect(updated?.last_run_status).toBe('failed');
  });

  it('ignores session events without a matching session', () => {
    expect(applyEventToSessions(sessions, event(1, 'run_started', { session_id: 's_nope', prompt: 'hi' }))).toBe(sessions);
  });
});

describe('seedChatState', () => {
  it('restores messages, tool output, pending approvals, agents and prior errors', () => {
    const current = seedChatState(detailFixture);
    expect(current.sessionId).toBe('s_ab12');
    expect(current.items.filter((item) => item.kind === 'message')).toHaveLength(3);
    const tool = current.items.find((item) => item.kind === 'tool');
    expect(tool?.kind === 'tool' ? tool.output : '').toContain('todo!()');
    expect(current.items.some((item) => item.kind === 'approval' && item.approval.id === 'ap_1')).toBe(true);
    expect(current.agents).toHaveLength(1);
    expect(current.running).toBe(false);
    expect(current.runStatus).toBe('completed');
  });

  it('marks a restored running run as streaming and exposes a failed run error', () => {
    const running = seedChatState({
      ...detailFixture,
      running: true,
      runs: [{ ...detailFixture.runs[0]!, status: 'running', finished_at: null }],
    });
    expect(running.running).toBe(true);
    expect(running.items.some((item) => item.kind === 'message' && item.streaming)).toBe(true);

    const failed = seedChatState({
      ...detailFixture,
      running: false,
      runs: [{ ...detailFixture.runs[0]!, status: 'failed', error: 'provider 500' }],
    });
    expect(failed.runError?.message).toBe('provider 500');
  });

  it('adds an interrupted notice for recovered runs', () => {
    const recovered = seedChatState({
      ...detailFixture,
      runs: [{ ...detailFixture.runs[0]!, status: 'interrupted' }],
    });
    expect(recovered.items.some((item) => item.kind === 'interrupted')).toBe(true);
  });

  it('restores command exit codes and streams from the stored report format', () => {
    const report = [
      '$ cargo test -p auth',
      'cwd: .',
      'exit code: 101',
      'duration: 4.82s',
      '',
      '--- stdout (12 bytes) ---',
      'running 1 test',
      '',
      '--- stderr (8 bytes) ---',
      'assert!',
    ].join('\n');
    const detail = {
      ...detailFixture,
      messages: [
        {
          ...detailFixture.messages[0]!,
          role: 'assistant' as const,
          content: '',
          tool_calls: [{ id: 'c1', name: 'run_command', arguments: '{"command":"cargo test -p auth","cwd":"."}' }],
        },
        {
          ...detailFixture.messages[0]!,
          id: 2,
          role: 'tool' as const,
          tool_call_id: 'c1',
          name: 'run_command',
          content: report,
        },
      ],
      approvals: [],
    };
    const current = seedChatState(detail);
    const command = current.items.find((item) => item.kind === 'command');
    expect(command).toMatchObject({
      kind: 'command',
      stdout: 'running 1 test',
      stderr: 'assert!',
      exitCode: 101,
      durationMs: 4820,
      status: 'error',
      output: '',
    });
  });

  it('pairs tool results with tool calls and keeps command output', () => {
    const detail = {
      ...detailFixture,
      messages: [
        {
          ...detailFixture.messages[0]!,
          role: 'assistant' as const,
          content: '',
          tool_calls: [{ id: 'c1', name: 'run_command', arguments: '{"command":"ls","cwd":"."}' }],
        },
        {
          ...detailFixture.messages[0]!,
          id: 2,
          role: 'tool' as const,
          tool_call_id: 'c1',
          name: 'run_command',
          content: 'src\nCargo.toml',
        },
      ],
      approvals: [],
    };
    const current = seedChatState(detail);
    const command = current.items.find((item) => item.kind === 'command');
    expect(command?.kind === 'command' ? command.command : '').toBe('ls');
    expect(command?.kind === 'command' ? command.output : '').toContain('Cargo.toml');
  });
});