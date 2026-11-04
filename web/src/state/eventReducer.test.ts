import { describe, expect, it } from 'vitest';
import type { KarucodeEvent, SessionSummary } from '../api/types';
import { emptyChatState, seedChatState, type ChatState, type MessageItem } from './chat';
import { applyEventToChat, applyEventToSessions } from './eventReducer';
import {
  detailFixture,
  NOW,
  queuedFixture,
  secondSummaryFixture,
  summaryFixture,
  withoutDraft,
} from '../test/fixtures';

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

  it('drops the abandoned attempt when the runtime restarts a dead stream', () => {
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(current, event(2, 'reasoning_delta', { delta: 'thinking hard' }));
    current = applyEventToChat(current, event(3, 'text_delta', { delta: 'half an ans' }));

    current = applyEventToChat(
      current,
      event(4, 'stream_retry', {
        attempt: 1,
        max_attempts: 4,
        reason: 'provider error: invalid response body: error decoding response body',
      }),
    );

    // Nothing of the attempt that died may survive: it would be duplicated by
    // the retry's own deltas.
    expect(
      current.items.some(
        (item) =>
          item.kind === 'message' &&
          item.role === 'assistant' &&
          (item.content === 'half an ans' || item.reasoning === 'thinking hard'),
      ),
    ).toBe(false);
    const notice = current.items.find((item) => item.kind === 'notice');
    expect(notice?.kind === 'notice' ? notice.message : '').toContain('retrying');
    expect(notice?.kind === 'notice' ? notice.level : '').toBe('warn');
    expect(current.running).toBe(true);

    // The retry streams into a fresh segment instead of appending to the dead one.
    current = applyEventToChat(current, event(5, 'text_delta', { delta: 'the whole answer' }));
    expect(lastMessage(current).content).toBe('the whole answer');
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

  it('keeps the images a tool attached so the chat can preview them', () => {
    const image = { media_type: 'image/png', data: 'iVBORw0KGgo=', bytes: 8, name: 'shot.png' };
    let current = applyEventToChat(
      state(),
      event(1, 'tool_call_started', { call_id: 'call_img', name: 'read_file', arguments: { path: 'shot.png' } }),
    );
    current = applyEventToChat(
      current,
      event(2, 'tool_call_finished', {
        call_id: 'call_img',
        name: 'read_file',
        ok: true,
        output: 'shot.png — image/png',
        truncated: false,
        duration_ms: 7,
        images: [image],
      }),
    );
    const tool = current.items.find((item) => item.kind === 'tool');
    expect(tool?.kind === 'tool' ? tool.images : []).toEqual([image]);

    // A duplicate finish event that carries no images must not blank the preview.
    current = applyEventToChat(
      current,
      event(3, 'tool_call_finished', {
        call_id: 'call_img',
        name: 'read_file',
        ok: true,
        output: 'shot.png — image/png',
        truncated: false,
        duration_ms: 7,
      }),
    );
    const after = current.items.find((item) => item.kind === 'tool');
    expect(after?.kind === 'tool' ? after.images : []).toEqual([image]);
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

  it('accumulates cached prompt tokens so the hit rate is visible', () => {
    let current = applyEventToChat(
      state(),
      event(1, 'usage', { input_tokens: 100, output_tokens: 4, cached_tokens: 90 }),
    );
    current = applyEventToChat(
      current,
      event(2, 'usage', { input_tokens: 100, output_tokens: 2, cached_tokens: 98 }),
    );
    expect(current.usage).toEqual({ input_tokens: 200, output_tokens: 6, cached_tokens: 188 });
  });

  it('keeps the newest call prompt size apart from the running total', () => {
    // The context pill needs one request's prompt size; the HUD needs the cost of
    // every call so far. Summing one into the other is how a 900k-token run would
    // claim to be 700% through a 128k window.
    let current = applyEventToChat(state(), event(1, 'usage', { input_tokens: 40_000, output_tokens: 4 }));
    expect(current.lastPromptTokens).toBe(40_000);
    current = applyEventToChat(current, event(2, 'usage', { input_tokens: 100_000, output_tokens: 2 }));
    expect(current.lastPromptTokens).toBe(100_000);
    expect(current.usage?.input_tokens).toBe(140_000);
  });

  it('ignores a zero prompt count instead of blanking the pill mid-run', () => {
    // Some gateways omit `prompt_tokens` on the final chunk; that is "not
    // reported", not "an empty context".
    let current = applyEventToChat(state(), event(1, 'usage', { input_tokens: 40_000, output_tokens: 4 }));
    current = applyEventToChat(current, event(2, 'usage', { input_tokens: 0, output_tokens: 2 }));
    expect(current.lastPromptTokens).toBe(40_000);
  });

  it('leaves the prompt size unset until a call reports one', () => {
    expect(state().lastPromptTokens).toBeNull();
    const current = applyEventToChat(
      state(),
      envelope(1, { type: 'usage', input_tokens: 9_000, output_tokens: 1, agent_id: 'a_1' } as Partial<KarucodeEvent>),
    );
    // A subagent's usage says nothing about the main conversation's window.
    expect(current.lastPromptTokens).toBeNull();
  });

  it('leaves the hit rate unknown when the provider does not report it', () => {
    let current = applyEventToChat(state(), event(1, 'usage', { input_tokens: 10, output_tokens: 1 }));
    current = applyEventToChat(
      current,
      event(2, 'usage', { input_tokens: 10, output_tokens: 1, cached_tokens: 5 }),
    );
    expect(current.usage?.cached_tokens).toBe(5);
    // A later call that reports nothing must not reset the accumulated figure.
    current = applyEventToChat(current, event(3, 'usage', { input_tokens: 10, output_tokens: 1 }));
    expect(current.usage?.cached_tokens).toBe(5);
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
  it('records a lag marker as a warning notice without touching the run', () => {
    const current = applyEventToChat(
      { ...state(), running: true, runStatus: 'running' },
      event(7, 'stream_lagged', { skipped: 3 }),
    );
    const notice = current.items.find((item) => item.kind === 'notice');
    expect(notice?.kind === 'notice' ? notice.message : '').toContain('3 update(s) were dropped');
    // It describes the transport: the run itself is unchanged.
    expect(current.running).toBe(true);
    expect(current.runStatus).toBe('running');
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

  it('says so at the head of the transcript when the server truncated history', () => {
    const current = seedChatState({
      ...detailFixture,
      messages_truncated: true,
      messages_limit: 5000,
    });

    const first = current.items[0];
    expect(first?.kind).toBe('notice');
    expect(first?.kind === 'notice' ? first.message : '').toBe(
      'Earlier history is not loaded.',
    );
    // The notice leads the transcript; the real messages follow it.
    expect(current.items.filter((item) => item.kind === 'message')).toHaveLength(3);
  });

  it('adds no truncation notice when the whole history fits', () => {
    const current = seedChatState({ ...detailFixture, messages_truncated: false, messages_limit: 5000 });
    expect(current.items.some((item) => item.kind === 'notice' && item.id === 'notice:history-truncated')).toBe(false);
  });

  it('still explains the truncation when the server omits the window size', () => {
    const current = seedChatState({ ...detailFixture, messages_truncated: true });
    const first = current.items[0];
    expect(first?.kind === 'notice' ? first.message : '').toBe(
      'Earlier history is not loaded.',
    );
  });
});

describe('queued messages', () => {
  it('normalises a frame that omits the draft, so every row can render one', () => {
    // `queued_messages` serialises the raw `QueuedMessage`, where `draft` is
    // `skip_serializing_if = "Option::is_none"` - absent whenever it equals
    // `content`, which is the normal no-attachment case. Only
    // `GET .../queues` goes through `public_view()`. The reducer fills the gap in
    // at the boundary so no component has to know.
    const queued = applyEventToChat(state(), {
      ...event(3, 'queued_messages'),
      session_id: 's_ab12',
      steers: [withoutDraft(queuedFixture({ id: 'steer_1', content: 'also check the fixtures' }))],
      follow_ups: [],
    } as unknown as KarucodeEvent);

    expect(queued.queued).toHaveLength(1);
    expect(queued.queued[0]?.draft).toBe('also check the fixtures');
    // An explicitly empty draft is a draft (an attachments-only message), not a
    // missing field: it must not be replaced by the rendered content.
    const withEmpty = applyEventToChat(state(), {
      ...event(4, 'queued_messages'),
      session_id: 's_ab12',
      steers: [queuedFixture({ id: 'steer_1', draft: '', content: '<file>body</file>' })],
      follow_ups: [],
    } as unknown as KarucodeEvent);
    expect(withEmpty.queued[0]?.draft).toBe('');
  });

  it('replaces the pending list from the whole-queue frame', () => {
    const queued = applyEventToChat(state(), {
      ...event(3, 'queued_messages'),
      session_id: 's_ab12',
      steers: [queuedFixture({ id: 'steer_1' })],
      follow_ups: [queuedFixture({ id: 'followup_1', kind: 'follow_up' })],
    } as unknown as KarucodeEvent);

    // The frame carries both queues, so the list is exactly what it says.
    expect(queued.queued.map((entry) => entry.id)).toEqual(['steer_1', 'followup_1']);
  });

  it('empties the pending list when the run drains it', () => {
    const seeded: ChatState = {
      ...state(),
      queued: [queuedFixture({ id: 'steer_1' })],
    };
    const drained = applyEventToChat(seeded, {
      ...event(4, 'queued_messages'),
      session_id: 's_ab12',
      steers: [],
      follow_ups: [],
    } as unknown as KarucodeEvent);
    expect(drained.queued).toEqual([]);
  });

  it('drops a steer the moment the run applies it', () => {
    const seeded: ChatState = {
      ...state(),
      queued: [queuedFixture({ id: 'steer_1' })],
    };
    const applied = applyEventToChat(seeded, {
      ...event(5, 'steer_applied'),
      session_id: 's_ab12',
      steer_id: 'steer_1',
      content: 'also check the fixtures',
    } as unknown as KarucodeEvent);
    // The transcript row is not added here (the message arrives on the next
    // request), but the optimistic pending row must not outlive its acceptance.
    expect(applied.queued).toEqual([]);
  });

  it('leaves other queued messages alone when one steer lands', () => {
    const seeded: ChatState = {
      ...state(),
      queued: [
        queuedFixture({ id: 'steer_1' }),
        queuedFixture({ id: 'followup_1', kind: 'follow_up' }),
      ],
    };
    const applied = applyEventToChat(seeded, {
      ...event(6, 'steer_applied'),
      session_id: 's_ab12',
      steer_id: 'steer_1',
      content: 'first',
    } as unknown as KarucodeEvent);
    expect(applied.queued.map((entry) => entry.id)).toEqual(['followup_1']);
  });

  it('ignores a queue frame from another session', () => {
    const seeded: ChatState = {
      ...state(),
      queued: [queuedFixture({ id: 'steer_1' })],
    };
    const other = applyEventToChat(seeded, {
      ...event(7, 'queued_messages'),
      session_id: 's_other',
      steers: [],
      follow_ups: [],
    } as unknown as KarucodeEvent);
    expect(other).toBe(seeded);
  });
});