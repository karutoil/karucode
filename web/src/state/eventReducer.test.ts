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

  it('atomically replaces, rather than appends, a run’s final normalized live text', () => {
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(current, event(2, 'reasoning_delta', { delta: 'keep this reasoning' }));
    current = applyEventToChat(current, event(3, 'text_delta', { delta: 'raw repeated raw repeated' }));

    current = applyEventToChat(current, event(4, 'text_replaced', {
      run_id: 'r_x', content: 'final normalized answer',
    }));

    const assistants = current.items.filter(
      (item) => item.kind === 'message' && item.role === 'assistant' && item.runId === 'r_x',
    );
    expect(assistants).toHaveLength(1);
    expect(assistants[0]?.kind === 'message' ? assistants[0].content : '').toBe('final normalized answer');
    expect(assistants[0]?.kind === 'message' ? assistants[0].reasoning : '').toBe('keep this reasoning');
    expect(assistants[0]?.kind === 'message' ? assistants[0].streaming : false).toBe(true);
  });

  it('supports an empty text replacement without touching another run or session', () => {
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(current, event(2, 'text_delta', { delta: 'temporary' }));
    const unchanged = applyEventToChat(current, envelope(3, {
      type: 'text_replaced', run_id: 'r_other', session_id: 's_ab12', content: 'wrong run',
    } as Partial<KarucodeEvent>));
    expect(unchanged.items).toBe(current.items);

    current = applyEventToChat(current, event(4, 'text_replaced', { run_id: 'r_x', content: '' }));
    const answer = current.items.find(
      (item) => item.kind === 'message' && item.role === 'assistant' && item.runId === 'r_x',
    );
    expect(answer?.kind === 'message' ? answer.content : 'missing').toBe('');

    const otherSession = applyEventToChat(current, envelope(5, {
      type: 'text_replaced', run_id: 'r_x', session_id: 's_other', content: 'foreign',
    } as Partial<KarucodeEvent>));
    expect(otherSession).toBe(current);
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

  it('says why a run stopped at a limit instead of blaming a configured one', () => {
    // `limit_reached` used to mean one thing: an iteration budget the operator
    // had configured. It now also covers the wall clock and the loop guard, which
    // nobody configured - so the notice reads the runtime's own reason when it
    // sends one.
    const guard = applyEventToChat(
      state(),
      event(1, 'run_finished', {
        status: 'limit_reached',
        error:
          'stopped: the last 4 turns produced no new information - every call returned an answer this run already had, or was refused as a repeat.',
      }),
    );
    const notice = guard.items.find((item) => item.kind === 'notice');
    expect(notice?.kind === 'notice' ? notice.message : '').toContain('no new information');
    expect(notice?.kind === 'notice' ? notice.level : '').toBe('warn');
    expect(
      guard.items.some(
        (item) =>
          item.kind === 'notice' && item.message.includes('a configured limit'),
      ),
    ).toBe(false);

    // The runtime streams the same sentence as a notice while the run ends, so
    // the transcript must not show it twice.
    const withNotice = applyEventToChat(
      applyEventToChat(state(), {
        ...event(3, 'notice', { level: 'warn', message: 'stopped: the loop guard fired.' }),
      }),
      event(4, 'run_finished', { status: 'limit_reached', error: 'stopped: the loop guard fired.' }),
    );
    expect(
      withNotice.items.filter(
        (item) => item.kind === 'notice' && item.message === 'stopped: the loop guard fired.',
      ),
    ).toHaveLength(1);

    // An older server that sends no reason still gets a true sentence.
    const bare = applyEventToChat(state(), event(2, 'run_finished', { status: 'limit_reached' }));
    const fallback = bare.items.find((item) => item.kind === 'notice');
    expect(fallback?.kind === 'notice' ? fallback.message : '').toContain('wall-clock ceiling');
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
    // A follow-up has no steer_applied event. The whole-queue frame is also its
    // tombstone, so a delayed HTTP acceptance cannot put it back in the panel.
    expect(drained.retiredQueuedIds).toContain('steer_1');
  });

  it('drops a steer the moment the run applies it, and shows it where it landed', () => {
    const seeded: ChatState = {
      ...state(),
      items: [
        {
          kind: 'message',
          id: 'stream:r_x',
          at: NOW,
          role: 'assistant',
          content: 'working on it',
          reasoning: '',
          streaming: true,
          runId: 'r_x',
          agentId: null,
        },
      ],
      queued: [queuedFixture({ id: 'steer_1' })],
    };
    const applied = applyEventToChat(seeded, {
      ...event(5, 'steer_applied'),
      session_id: 's_ab12',
      run_id: 'r_x',
      steer_id: 'steer_1',
      content: 'also check the fixtures',
    } as unknown as KarucodeEvent);
    // The pending row must not outlive its acceptance…
    expect(applied.queued).toEqual([]);
    expect(applied.retiredQueuedIds).toContain('steer_1');
    // …and the steer is now a user turn, where the model read it: the runtime
    // persists it as an ordinary user message, so a reload shows the same row.
    const rows = applied.items.filter((item) => item.kind === 'message');
    expect(rows.map((row) => (row.kind === 'message' ? row.role : ''))).toEqual(['assistant', 'user']);
    const steer = rows[rows.length - 1];
    expect(steer?.kind === 'message' ? steer.content : '').toBe('also check the fixtures');
    expect(steer?.kind === 'message' ? steer.id : '').toBe('user:steer:steer_1');
    // The prose that was streaming when the steer landed is finished prose now.
    expect(rows[0]?.kind === 'message' ? rows[0].streaming : true).toBe(false);
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

/**
 * The three ways the transcript used to lie about a run's state:
 * a warning that read as a failed run, a stray frame that re-armed the spinner,
 * and a frame applied twice. Each test states the user-visible symptom, not the
 * reducer field, so a future refactor cannot quietly reintroduce it.
 */
describe('applyEventToChat — state truthfulness', () => {
  it('drops the failed-run banner when the run that warned then succeeded', () => {
    // `Event::Error` is not terminal: the runtime raised this one for a steering
    // message it could not persist (crates/kc-runtime/src/agent.rs:1273) and kept
    // working. The old reducer left the banner up, so a successful answer sat
    // under a red "Run failed" until the user dismissed it by hand.
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(
      current,
      event(2, 'error', {
        message: 'could not persist a steering message: database is locked',
        kind: 'store',
      }),
    );
    expect(current.runError?.kind).toBe('store');

    current = applyEventToChat(current, event(3, 'run_finished', { status: 'completed' }));

    expect(current.runError).toBeNull();
    expect(current.running).toBe(false);
    expect(current.runStatus).toBe('completed');
  });

  it('keeps the run_finished message when the run really did fail', () => {
    // The mid-run warning is superseded, not merged: the terminal frame is the
    // only one that knows why the run died.
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(
      current,
      event(2, 'error', { message: 'stream hiccup', kind: 'provider' }),
    );
    current = applyEventToChat(
      current,
      event(3, 'run_finished', { status: 'failed', error: 'provider exploded' }),
    );

    expect(current.runError).toEqual({ runId: 'r_x', message: 'provider exploded', kind: null });
  });

  it('does not carry a banner from a cancelled run into the next one', () => {
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(
      current,
      event(2, 'error', { message: 'could not persist a delivered message', kind: 'store' }),
    );
    // The (fake) cancelled run. The cancel is the truth about how this run ended;
    // a banner naming it would outlive the reason for it.
    current = applyEventToChat(current, event(3, 'run_cancelled', { run_id: 'r_x' }));
    expect(current.runError).toBeNull();

    current = applyEventToChat(current, event(4, 'run_started', { prompt: 'Again' }));
    expect(current.runError).toBeNull();
    expect(current.running).toBe(true);
  });

  it('leaves a banner from another run alone', () => {
    // Two tabs, one session: run r_y's failure is not something run r_x ending
    // can declare settled.
    let current: ChatState = {
      ...state(),
      running: true,
      runId: 'r_x',
      runError: { runId: 'r_y', message: 'the other tab failed', kind: null },
    };
    current = applyEventToChat(current, event(1, 'run_finished', { run_id: 'r_x', status: 'completed' }));
    expect(current.runError?.message).toBe('the other tab failed');
  });

  it('ignores a delta that arrives after its own run finished', () => {
    // A duplicated frame, or a late delivery on a second stream handle, used to
    // append a second answer bubble and set `running: true` again - with no
    // terminal frame left to lower it, so the chat said "Streaming" and the
    // composer queued instead of sending until the next resync.
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(current, event(2, 'text_delta', { delta: 'the answer' }));
    current = applyEventToChat(current, event(3, 'run_finished', { status: 'completed' }));
    const settled = current;

    current = applyEventToChat(current, event(4, 'text_delta', { delta: 'late' }));

    expect(current.running).toBe(false);
    expect(current.runStatus).toBe('completed');
    expect(current.items).toHaveLength(settled.items.length);
    const answers = current.items.filter(
      (item) => item.kind === 'message' && item.role === 'assistant',
    );
    expect(answers).toHaveLength(1);
    expect(answers[0]?.kind === 'message' ? answers[0].content : '').toBe('the answer');
  });

  it('ignores a reasoning delta that arrives after its own run finished', () => {
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(current, event(2, 'run_finished', { status: 'completed' }));
    const settled = current;

    current = applyEventToChat(current, event(3, 'reasoning_delta', { delta: 'still thinking' }));

    expect(current.items).toBe(settled.items);
    expect(current.running).toBe(false);
  });

  it('applies a duplicated frame once', () => {
    // `text_delta` appends, so a replayed frame doubles the words. `seq` is
    // monotonic per server process, which is what makes the copy recognisable.
    const frame = event(7, 'text_delta', { delta: 'once' });
    let current = applyEventToChat(state(), event(6, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(current, frame);
    current = applyEventToChat(current, frame);

    const answers = current.items.filter(
      (item) => item.kind === 'message' && item.role === 'assistant',
    );
    expect(answers).toHaveLength(1);
    expect(answers[0]?.kind === 'message' ? answers[0].content : '').toBe('once');

    // And an older frame than the watermark is a replay too, not a rewind.
    const before = current;
    const older = applyEventToChat(current, event(5, 'notice', { level: 'info', message: 'stale' }));
    expect(older).toBe(before);
  });

  it('keeps the dedupe working after a frame that carries no cursor', () => {
    // A frame with no usable `seq` is deliberately applied (losing a real event
    // is worse than drawing one twice), but it must not advance the watermark:
    // `Math.max(lastSeq, undefined)` is `NaN`, and every later `seq <= NaN` is
    // false, so one cursorless frame silently switched the dedupe off for good.
    const cursorless = {
      ts_ms: NOW,
      session_id: 's_ab12',
      run_id: 'r_x',
      type: 'notice',
      level: 'info',
      message: 'from an older server',
    } as unknown as KarucodeEvent;

    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(current, cursorless);
    expect(
      current.items.some((item) => item.kind === 'notice' && item.message === 'from an older server'),
    ).toBe(true);
    expect(current.lastSeq).toBe(1);

    current = applyEventToChat(current, event(5, 'text_delta', { delta: 'once' }));
    const settled = current;
    expect(settled.lastSeq).toBe(5);
    current = applyEventToChat(current, event(5, 'text_delta', { delta: 'once' }));

    expect(current).toBe(settled);
    const answers = current.items.filter(
      (item) => item.kind === 'message' && item.role === 'assistant',
    );
    expect(answers).toHaveLength(1);
    expect(answers[0]?.kind === 'message' ? answers[0].content : '').toBe('once');
  });

  it('still applies the replay paths that are exempt from the watermark', () => {
    let current = applyEventToChat(state(), event(9, 'run_started', { prompt: 'Hi' }));

    // `stream_lagged` is stamped with the last *dropped* seq, so its cursor can be
    // older than what is applied - `api/events.ts` deliberately lets it past its
    // own dedupe, and the hole it reports must reach the transcript.
    current = applyEventToChat(current, event(5, 'stream_lagged', { skipped: 3 }));
    expect(current.items.some((item) => item.kind === 'notice')).toBe(true);

    // A frame with no usable cursor is never dropped: an older server that sends
    // `seq: 0` would otherwise lose every event after the first.
    const noCursor = applyEventToChat(current, event(0, 'notice', { level: 'info', message: 'old server' }));
    expect(noCursor.items.length).toBe(current.items.length + 1);

    // The snapshot reseeds the watermark, so the frames `drainRefreshQueue`
    // replays after it (collected while the read was in flight) still apply even
    // though the stream had already moved past them.
    const seeded = seedChatState(detailFixture);
    expect(seeded.lastSeq).toBe(0);
    const replayed = applyEventToChat(seeded, event(4, 'notice', { level: 'info', message: 'queued while reading' }));
    expect(replayed.items.some((item) => item.kind === 'notice' && item.message === 'queued while reading')).toBe(true);
  });
});

describe('applyEventToChat — streaming hot paths', () => {
  /** A live transcript with a user turn, a finished tool card, and a streaming answer on the tail. */
  function streamingTranscript(): ChatState {
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(current, event(2, 'text_delta', { delta: 'first' }));
    current = applyEventToChat(
      current,
      event(3, 'tool_call_started', { call_id: 'call_1', name: 'read_file', arguments: {} }),
    );
    current = applyEventToChat(
      current,
      event(4, 'tool_call_finished', { call_id: 'call_1', name: 'read_file', ok: true, output: 'x' }),
    );
    return applyEventToChat(current, event(5, 'text_delta', { delta: 'hello' }));
  }

  it('extends the streaming tail and leaves every earlier row identical', () => {
    const before = streamingTranscript();
    expect(before.items).toHaveLength(4);

    const next = applyEventToChat(before, event(6, 'text_delta', { delta: ' world' }));

    // Every row before the tail is the *same object*: a transcript is thousands
    // of rows and a delta arrives per token, so re-mapping it per token made the
    // renderer diff the whole conversation for one word.
    expect(next.items).toHaveLength(before.items.length);
    for (let i = 0; i < before.items.length - 1; i += 1) {
      expect(next.items[i]).toBe(before.items[i]);
    }
    const tail = next.items[next.items.length - 1];
    expect(tail).not.toBe(before.items[before.items.length - 1]);
    expect(tail?.kind === 'message' ? tail.content : '').toBe('hello world');
  });

  it('extends the reasoning tail without copying the rest of the transcript', () => {
    const before = streamingTranscript();
    const next = applyEventToChat(before, event(6, 'reasoning_delta', { delta: 'thinking' }));

    for (let i = 0; i < before.items.length - 1; i += 1) {
      expect(next.items[i]).toBe(before.items[i]);
    }
    const tail = next.items[next.items.length - 1];
    expect(tail?.kind === 'message' ? tail.reasoning : '').toBe('thinking');
    // Reasoning stays on the same segment as the text it accompanies.
    expect(tail?.kind === 'message' ? tail.content : '').toBe('hello');
    expect(next.items).toHaveLength(before.items.length);
  });

  it('opens a new segment when the tail is no longer streaming', () => {
    // The tail check is the whole lookup now, so the "tail is closed" case has to
    // still start a fresh segment rather than append to a settled one.
    let current = streamingTranscript();
    current = applyEventToChat(current, event(6, 'notice', { level: 'info', message: 'a note' }));
    const closed = current;
    const next = applyEventToChat(closed, event(7, 'text_delta', { delta: 'later' }));

    expect(next.items).toHaveLength(closed.items.length + 1);
    const tail = next.items[next.items.length - 1];
    expect(tail?.kind === 'message' ? tail.content : '').toBe('later');
  });

  it('appends command output to the one card that owns the call id', () => {
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'run it' }));
    current = applyEventToChat(current, event(2, 'command_started', { call_id: 'c1', command: 'ls', cwd: '/w' }));
    current = applyEventToChat(current, event(3, 'notice', { level: 'info', message: 'noise after the card' }));
    const before = current;
    const cardIndex = before.items.findIndex((item) => item.kind === 'command');

    const next = applyEventToChat(current, event(4, 'command_output', { call_id: 'c1', chunk: 'a\n', stream: 'stdout' }));
    const again = applyEventToChat(next, event(5, 'command_output', { call_id: 'c1', chunk: 'b\n', stream: 'stderr' }));

    // The rows around the card are shared, even though the card is not the tail.
    for (let i = 0; i < before.items.length; i += 1) {
      if (i === cardIndex) continue;
      expect(again.items[i]).toBe(before.items[i]);
    }
    const card = again.items[cardIndex];
    expect(card?.kind === 'command' ? card.stdout : null).toBe('a\n');
    expect(card?.kind === 'command' ? card.stderr : null).toBe('b\n');
    expect(again.items).toHaveLength(before.items.length);
  });

  it('replaces a streaming answer that is not the last row, and only that row', () => {
    // `text_replaced` arrives with the run's final normalised text; the answer is
    // found by scanning back, which has to work after a notice landed behind it.
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(current, event(2, 'text_delta', { delta: 'partial' }));
    current = applyEventToChat(current, event(3, 'notice', { level: 'info', message: 'a note' }));
    const before = current;
    const answerIndex = before.items.findIndex(
      (item) => item.kind === 'message' && item.role === 'assistant',
    );

    const next = applyEventToChat(current, event(4, 'text_replaced', { content: 'final answer' }));

    expect(next.items).toHaveLength(before.items.length);
    for (let i = 0; i < before.items.length; i += 1) {
      if (i === answerIndex) continue;
      expect(next.items[i]).toBe(before.items[i]);
    }
    const answer = next.items[answerIndex];
    expect(answer?.kind === 'message' ? answer.content : '').toBe('final answer');
  });

  it('finishes the tool card it was told about, not the newest one', () => {
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(current, event(2, 'tool_call_started', { call_id: 'a', name: 'read_file', arguments: {} }));
    current = applyEventToChat(current, event(3, 'tool_call_started', { call_id: 'b', name: 'grep', arguments: {} }));
    const before = current;

    const next = applyEventToChat(
      current,
      event(4, 'tool_call_finished', { call_id: 'a', name: 'read_file', ok: true, output: 'contents' }),
    );

    const first = next.items.find((item) => item.kind === 'tool' && item.callId === 'a');
    const second = next.items.find((item) => item.kind === 'tool' && item.callId === 'b');
    expect(first?.kind === 'tool' ? first.output : '').toBe('contents');
    expect(first?.kind === 'tool' ? first.status : '').toBe('ok');
    // The other card keeps its identity: finishing one call must not copy it.
    expect(second).toBe(before.items.find((item) => item.kind === 'tool' && item.callId === 'b'));
  });

  it('resolves the approval it was told about and shares every other row', () => {
    let current = applyEventToChat(state(), event(1, 'run_started', { prompt: 'Hi' }));
    current = applyEventToChat(current, event(2, 'tool_call_started', { call_id: 'a', name: 'write_file', arguments: {} }));
    current = applyEventToChat(current, event(3, 'notice', { level: 'info', message: 'a note' }));
    const approvalEvent = event(4, 'approval_requested', {
      approval: {
        id: 'ap_1',
        kind: 'file_write',
        title: 'Write?',
        detail: 'write_file',
        payload: {},
        session_id: 's_ab12',
        run_id: 'r_x',
        agent_id: null,
      },
    });
    current = applyEventToChat(current, approvalEvent);
    const before = current;
    const approvalIndex = before.items.findIndex((item) => item.kind === 'approval');

    const next = applyEventToChat(
      current,
      event(5, 'approval_resolved', { approval_id: 'ap_1', decision: 'allow_once' }),
    );

    expect(next.items).toHaveLength(before.items.length);
    for (let i = 0; i < before.items.length; i += 1) {
      if (i === approvalIndex) continue;
      expect(next.items[i]).toBe(before.items[i]);
    }
    const resolved = next.items[approvalIndex];
    expect(resolved?.kind === 'approval' ? resolved.approval.status : '').toBe('resolved');
    expect(resolved?.kind === 'approval' ? resolved.approval.decision : null).toBe('allow_once');
  });
});
