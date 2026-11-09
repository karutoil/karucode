import { describe, expect, it } from 'vitest';
import type { AgentSummary, KarucodeEvent, SessionSummary, StoredMessage } from '../api/types';
import {
  activityFromMessages,
  agentEventId,
  applyAgentEvent,
  applyAgentRegistry,
  emptyActivity,
  mergeActivities,
  mergeAgentList,
} from './agentConsole';

const NOW = 1_700_000_000_000;

function event<T extends KarucodeEvent['type']>(
  seq: number,
  type: T,
  fields: Record<string, unknown> = {},
): KarucodeEvent {
  return {
    seq,
    ts_ms: NOW + seq,
    session_id: 's_ab12',
    run_id: 'r_x',
    type,
    ...fields,
  } as KarucodeEvent;
}

function agentEvent<T extends KarucodeEvent['type']>(
  seq: number,
  type: T,
  agentId: string,
  fields: Record<string, unknown> = {},
): KarucodeEvent {
  return event(seq, type, { agent_id: agentId, ...fields });
}

function message(partial: Partial<StoredMessage> & Pick<StoredMessage, 'id' | 'role'>): StoredMessage {
  return {
    session_id: 's_ab12',
    run_id: 'r_x',
    agent_id: 'a_child',
    content: '',
    tool_calls: null,
    tool_call_id: null,
    name: null,
    created_at: NOW,
    ...partial,
  };
}

function summary(session: string, title: string): SessionSummary {
  return {
    id: session,
    title,
    workspace: `/work/${session}`,
    project_id: 'p_1',
    created_at: NOW,
    updated_at: NOW,
    message_count: 0,
    last_run_status: null,
    running: false,
  };
}

function agent(id: string, created: number): AgentSummary {
  return {
    id,
    session_id: 's_ab12',
    parent_run_id: 'r_x',
    parent_agent_id: null,
    name: id,
    role: 'worker',
    task: 'do the thing',
    status: 'running',
    model: 'gpt-5',
    iterations: 0,
    summary: '',
    error: null,
    owned_paths: [],
    created_at: created,
    updated_at: created,
    finished_at: null,
    background: false,
    note: '',
    progress: null,
    session_title: 'Session',
    workspace: '/work',
    project_id: 'p_1',
  };
}

describe('agent event routing', () => {
  it('uses the agent id stamped on the frame and ignores parent-run frames', () => {
    expect(agentEventId(agentEvent(1, 'text_delta', 'a_1', { delta: 'hi' }))).toBe('a_1');
    expect(agentEventId(event(2, 'text_delta', { delta: 'hi' }))).toBeNull();
  });

  it('keeps each agent on its own live feed', () => {
    const one = applyAgentEvent(
      emptyActivity(),
      event(1, 'text_delta', { agent_id: 'a_1', delta: 'alpha' }),
    );
    const two = applyAgentEvent(
      emptyActivity(),
      event(2, 'text_delta', { agent_id: 'a_2', delta: 'beta' }),
    );

    expect(one.items).toHaveLength(1);
    expect(two.items).toHaveLength(1);
    expect(one.items[0]?.kind === 'message' ? one.items[0].content : '').toBe('alpha');
    expect(two.items[0]?.kind === 'message' ? two.items[0].content : '').toBe('beta');

    // A second delta for the first agent extends ITS message, so two agents that
    // stream at the same time never interleave into one bubble.
    const continued = applyAgentEvent(
      one,
      event(3, 'text_delta', { agent_id: 'a_1', delta: '!' }),
    );
    expect(continued.items).toHaveLength(1);
    expect(continued.items[0]?.kind === 'message' ? continued.items[0].content : '').toBe('alpha!');
    expect(continued.items[0]?.kind === 'message' ? continued.items[0].streaming : false).toBe(true);
  });

  it('atomically replaces live subagent text without moving or settling tool activity', () => {
    let activity = applyAgentEvent(
      emptyActivity(),
      agentEvent(1, 'reasoning_delta', 'a_child', { delta: 'keep this reasoning' }),
    );
    activity = applyAgentEvent(
      activity,
      agentEvent(2, 'text_delta', 'a_child', { delta: 'raw repeated raw repeated' }),
    );
    activity = applyAgentEvent(
      activity,
      agentEvent(3, 'tool_call_started', 'a_child', {
        call_id: 'call_1', name: 'read_file', arguments: { path: 'src/lib.rs' },
      }),
    );
    const beforeForeignRun = applyAgentEvent(
      activity,
      agentEvent(4, 'text_replaced', 'a_child', {
        run_id: 'r_other', content: 'must not replace another run',
      }),
    );
    expect(beforeForeignRun).toBe(activity);

    activity = applyAgentEvent(
      activity,
      agentEvent(5, 'text_replaced', 'a_child', { content: 'final normalized answer' }),
    );
    expect(activity.items).toHaveLength(2);
    const answer = activity.items[0];
    const tool = activity.items[1];
    expect(answer?.kind === 'message' ? answer.content : 'missing').toBe('final normalized answer');
    expect(answer?.kind === 'message' ? answer.reasoning : '').toBe('keep this reasoning');
    expect(answer?.kind === 'message' ? answer.streaming : false).toBe(true);
    expect(answer?.kind === 'message' ? answer.agentId : null).toBe('a_child');
    expect(tool?.kind === 'tool' ? tool.status : null).toBe('running');
    expect(tool?.kind === 'tool' ? tool.output : null).toBe('');
    expect(activity.lastSeq).toBe(5);
  });

  it('builds a tool card from started/finished frames, including its output', () => {
    let activity = emptyActivity();
    activity = applyAgentEvent(
      activity,
      agentEvent(1, 'tool_call_started', 'a_1', {
        call_id: 'call_1',
        name: 'read_file',
        arguments: { path: 'src/lib.rs' },
      }),
    );
    activity = applyAgentEvent(
      activity,
      agentEvent(2, 'tool_call_finished', 'a_1', {
        call_id: 'call_1',
        name: 'read_file',
        ok: true,
        output: 'fn main() {}',
        truncated: false,
        duration_ms: 12,
      }),
    );

    expect(activity.items).toHaveLength(1);
    const tool = activity.items[0];
    if (tool?.kind !== 'tool') throw new Error('expected a tool item');
    expect(tool.status).toBe('ok');
    expect(tool.output).toBe('fn main() {}');
    expect(tool.durationMs).toBe(12);
  });

  it('marks running work as stopped when the agent finishes', () => {
    let activity = emptyActivity();
    activity = applyAgentEvent(
      activity,
      agentEvent(1, 'tool_call_started', 'a_1', {
        call_id: 'call_1',
        name: 'read_file',
        arguments: {},
      }),
    );
    activity = applyAgentEvent(
      activity,
      agentEvent(2, 'agent_finished', 'a_1', {
        status: 'cancelled',
        summary: 'cancelled by user',
      }),
    );
    const tool = activity.items[0];
    expect(tool?.kind === 'tool' ? tool.status : null).toBe('stopped');
  });

  it('adds a subagent approval to the transcript and resolves it', () => {
    let activity = emptyActivity();
    activity = applyAgentEvent(
      activity,
      agentEvent(1, 'approval_requested', 'a_1', {
        approval: {
          id: 'ap_1',
          kind: 'shell_command',
          title: 'Run cargo test',
          detail: 'cargo test',
          payload: {},
          session_id: 's_ab12',
          run_id: 'r_x',
          agent_id: 'a_1',
        },
      }),
    );
    activity = applyAgentEvent(
      activity,
      agentEvent(2, 'approval_resolved', 'a_1', {
        approval_id: 'ap_1',
        decision: 'allow_once',
      }),
    );
    const approval = activity.items[0];
    if (approval?.kind !== 'approval') throw new Error('expected an approval item');
    expect(approval.approval.status).toBe('resolved');
    expect(approval.approval.decision).toBe('allow_once');
  });
});

describe('agent registry', () => {
  const sessions = [summary('s_ab12', 'Review the harness')];

  it('adds a spawned agent with its session context and updates it', () => {
    let agents = applyAgentRegistry(
      [],
      event(1, 'agent_spawned', {
        agent_id: 'a_1',
        name: 'reviewer',
        role: 'reviewer',
        task: 'review it',
        model: 'gpt-5',
        parent_run_id: 'r_x',
      }),
      sessions,
    );
    expect(agents).toHaveLength(1);
    expect(agents[0]).toMatchObject({
      id: 'a_1',
      name: 'reviewer',
      status: 'running',
      session_title: 'Review the harness',
      workspace: '/work/s_ab12',
      project_id: 'p_1',
    });

    agents = applyAgentRegistry(
      agents,
      agentEvent(2, 'agent_progress', 'a_1', { iterations: 7, note: '' }),
      sessions,
    );
    expect(agents[0]?.iterations).toBe(7);

    agents = applyAgentRegistry(
      agents,
      agentEvent(3, 'agent_finished', 'a_1', {
        status: 'completed',
        summary: 'done',
      }),
      sessions,
    );
    expect(agents[0]).toMatchObject({
      status: 'completed',
      summary: 'done',
      finished_at: NOW + 3,
    });
  });

  it('creates a placeholder for an agent whose spawn frame was missed', () => {
    const agents = applyAgentRegistry(
      [],
      agentEvent(9, 'agent_finished', 'a_lost', { status: 'failed', summary: '' }),
      sessions,
    );
    expect(agents[0]).toMatchObject({ id: 'a_lost', status: 'failed', session_id: 's_ab12' });
  });

  it('keeps a pause that arrives before the list lands', () => {
    // On a cold tab the SSE stream is already open while `GET /api/agents` is
    // still in flight, so `agent_paused` can arrive first. The reducer used to
    // bail out on an unknown row, which lost the frame: the row was created by the
    // list read that followed, and it rendered `running` - a paused agent that
    // says it is spending. `agent_finished` already synthesised a row here; the
    // pause has to do the same.
    const agents = applyAgentRegistry(
      [],
      agentEvent(1, 'agent_paused', 'a_early', { paused: true }),
      sessions,
    );

    expect(agents).toHaveLength(1);
    expect(agents[0]).toMatchObject({
      id: 'a_early',
      status: 'paused',
      session_id: 's_ab12',
      session_title: 'Review the harness',
      // A placeholder is not a finished agent, and it holds no invented facts.
      finished_at: null,
      iterations: 0,
    });

    // A resume for the same early agent flips it back rather than being dropped.
    const resumed = applyAgentRegistry(
      agents,
      agentEvent(2, 'agent_paused', 'a_early', { paused: false }),
      sessions,
    );
    expect(resumed[0]?.status).toBe('running');

    // The list read that lands afterwards is the authority on the rest of the row.
    const merged = mergeAgentList(resumed, [
      { ...agent('a_early', NOW), status: 'running', name: 'reviewer', task: 'review it' },
    ]);
    expect(merged).toHaveLength(1);
    expect(merged[0]).toMatchObject({ id: 'a_early', name: 'reviewer', task: 'review it' });
  });

  it('keeps an early progress frame and an early note', () => {
    // Same rule for the other two frames: each one carries a fact (the iteration
    // count, what the agent says about itself) that a later list read cannot
    // recover, because the list only has the status.
    const withProgress = applyAgentRegistry(
      [],
      agentEvent(1, 'agent_progress', 'a_progress', { iterations: 4 }),
      sessions,
    );
    expect(withProgress[0]).toMatchObject({ id: 'a_progress', iterations: 4, status: 'running' });

    const withNote = applyAgentRegistry(
      [],
      agentEvent(1, 'agent_note', 'a_note', { note: 'reading the diff', progress: 0.5 }),
      sessions,
    );
    expect(withNote[0]).toMatchObject({
      id: 'a_note',
      note: 'reading the diff',
      progress: 0.5,
      status: 'running',
    });
  });

  it('still ignores an early frame from a session outside the open project', () => {
    // The other-project guard must survive the new placeholder path: the list is
    // scoped to the project, and a global SSE stream must not leak rows into it.
    const agents = applyAgentRegistry(
      [],
      event(1, 'agent_paused', {
        session_id: 's_other_project',
        agent_id: 'a_other',
        paused: true,
      }),
      sessions,
    );
    expect(agents).toEqual([]);
  });

  it('ignores a spawn from a session outside the open project', () => {
    const agents = applyAgentRegistry(
      [],
      event(1, 'agent_spawned', {
        session_id: 's_other_project',
        agent_id: 'a_other',
        name: 'reviewer',
        role: 'reviewer',
        task: 'elsewhere',
        model: 'gpt-5',
        parent_run_id: 'r_other',
      }),
      sessions,
    );
    expect(agents).toEqual([]);
  });

  it('merges a refreshed list without dropping live-created placeholders', () => {
    const merged = mergeAgentList(
      [agent('a_live', NOW)],
      [{ ...agent('a_server', NOW - 10), status: 'completed' }],
    );
    expect(merged.map((entry) => entry.id)).toEqual(['a_live', 'a_server']);
  });

  it('does not let a refreshed row lower an iteration count this page watched advance', () => {
    // The server row is written between iterations, so a list read that races a
    // progress frame carries an older count. Replacing the row wholesale made
    // the number fall back (to `0` for a run that had only just started).
    const merged = mergeAgentList(
      [{ ...agent('a_live', NOW), iterations: 9 }],
      [{ ...agent('a_live', NOW), iterations: 4 }],
    );
    expect(merged).toHaveLength(1);
    expect(merged[0]?.iterations).toBe(9);
  });
});

describe('stored agent transcripts', () => {
  it('rebuilds the task, the answer and its tool call', () => {
    const messages: StoredMessage[] = [
      message({ id: 1, role: 'user', content: 'review the diff' }),
      message({
        id: 2,
        role: 'assistant',
        content: '',
        tool_calls: [{ id: 'call_1', name: 'read_file', arguments: '{"path":"src/lib.rs"}' }],
      }),
      message({ id: 3, role: 'tool', tool_call_id: 'call_1', name: 'read_file', content: 'ok' }),
      message({ id: 4, role: 'assistant', content: 'looks fine' }),
    ];

    const activity = activityFromMessages(messages, 'a_child');
    expect(activity.items).toHaveLength(3);
    expect(activity.items[0]).toMatchObject({ kind: 'message', role: 'user', content: 'review the diff' });
    expect(activity.items[1]).toMatchObject({ kind: 'tool', name: 'read_file', output: 'ok' });
    expect(activity.items[2]).toMatchObject({ kind: 'message', role: 'assistant', content: 'looks fine' });
  });

  it('ignores messages that belong to another agent', () => {
    const activity = activityFromMessages(
      [
        message({ id: 1, role: 'user', content: 'mine' }),
        message({ id: 2, role: 'user', agent_id: 'a_other', content: 'theirs' }),
      ],
      'a_child',
    );
    expect(activity.items).toHaveLength(1);
    expect(activity.items[0]?.kind === 'message' ? activity.items[0].content : '').toBe('mine');
  });

  it('restores a stored thought so a reloaded transcript still shows it', () => {
    // A subagent's reasoning is persisted on its assistant row; opening the agent
    // after the fact (or reloading) must not turn the thought into prose-free
    // nothing.
    const activity = activityFromMessages(
      [
        message({ id: 1, role: 'user', content: 'review the diff' }),
        message({
          id: 2,
          role: 'assistant',
          content: 'looks fine',
          reasoning_content: 'Read the test names before judging the diff.',
        }),
      ],
      'a_child',
    );
    const answer = activity.items.find((item) => item.kind === 'message' && item.role === 'assistant');
    expect(answer?.kind === 'message' ? answer.reasoning : '').toBe(
      'Read the test names before judging the diff.',
    );
    expect(answer?.kind === 'message' ? answer.content : '').toBe('looks fine');
  });

  it('merges stored history with live frames without duplicating a call', () => {
    const stored = activityFromMessages(
      [
        message({ id: 1, role: 'user', content: 'task' }),
        message({
          id: 2,
          role: 'assistant',
          tool_calls: [{ id: 'call_1', name: 'read_file', arguments: '{}' }],
        }),
        message({ id: 3, role: 'tool', tool_call_id: 'call_1', content: 'stored' }),
      ],
      'a_child',
    );
    const live = applyAgentEvent(
      emptyActivity(),
      agentEvent(9, 'tool_call_finished', 'a_child', {
        call_id: 'call_1',
        name: 'read_file',
        ok: false,
        output: 'live',
        truncated: false,
        duration_ms: 5,
      }),
    );
    const merged = mergeActivities(stored, live);
    expect(merged.items).toHaveLength(2);
    const tool = merged.items.find((item) => item.kind === 'tool');
    expect(tool?.kind === 'tool' ? tool.output : '').toBe('live');
  });
});
