import { describe, expect, it } from 'vitest';
import type { ImageAttachment, RunRecord, RunStatus, SessionDetail, StoredMessage, ToolCall } from '../api/types';
import { NOW, detailFixture, omitFields, queuedFixture, storedMessage, wireAgentSummary } from '../test/fixtures';
import { mergeTranscriptRows, seedChatState, withStoredMessageIds } from './chat';
import type { ChatItem } from './chat';

const shot: ImageAttachment = {
  media_type: 'image/png',
  data: 'iVBORw0KGgo=',
  bytes: 8,
  name: 'shot.png',
};

function sessionWith(messages: StoredMessage[]): SessionDetail {
  return {
    ...detailFixture,
    messages,
    runs: [],
    agents: [],
    approvals: [],
    claims: [],
    running: false,
  };
}

const call: ToolCall = { id: 'call_1', name: 'read_file', arguments: '{"path":"shot.png"}' };

function assistantTurn(): StoredMessage {
  return storedMessage({ id: 1, role: 'assistant', tool_calls: [call] });
}

/** A settled (or still running) run row as the session snapshot carries it. */
function runRow(
  status: RunStatus,
  id: string,
  startedAt: number,
  finishedAt: number | null,
  error: string | null = null,
): RunRecord {
  return {
    id,
    session_id: detailFixture.session.id,
    status,
    model: 'gpt-4o-mini',
    prompt: 'run until something stops it',
    error,
    started_at: startedAt,
    finished_at: finishedAt,
  };
}

describe('seedChatState', () => {
  it('carries both queues off the snapshot, steers first', () => {
    // Steers land before follow-ups (a steer joins the turn in flight, a
    // follow-up gets its own run), so the pending list is in landing order.
    const state = seedChatState({
      ...detailFixture,
      running: true,
      queued_steers: [queuedFixture({ id: 'steer_1', kind: 'steer' })],
      queued_follow_ups: [
        queuedFixture({ id: 'followup_1', kind: 'follow_up', draft: 'then run the tests' }),
      ],
    });
    expect(state.queued.map((entry) => entry.id)).toEqual(['steer_1', 'followup_1']);
  });

  it('has an empty queue when the snapshot carries none', () => {
    expect(seedChatState({ ...detailFixture, running: false }).queued).toEqual([]);
  });

  it('carries the stored tool images onto the card so a reload still previews them', () => {
    const toolRow: StoredMessage = {
      ...storedMessage({
        id: 2,
        role: 'tool',
        tool_call_id: 'call_1',
        name: 'read_file',
        content: 'shot.png — image/png (8 B)',
      }),
      images: [shot],
    };
    const state = seedChatState(sessionWith([assistantTurn(), toolRow]));

    const tool = state.items.find((item) => item.kind === 'tool');
    expect(tool?.kind === 'tool' ? tool.images : []).toEqual([shot]);
    expect(tool?.kind === 'tool' ? tool.output : '').toContain('image/png');
  });

  it('leaves images out for a text-only tool result', () => {
    const toolRow = storedMessage({
      id: 2,
      role: 'tool',
      tool_call_id: 'call_1',
      name: 'read_file',
      content: 'a.txt — lines 1-1 of 1',
    });
    const state = seedChatState(sessionWith([assistantTurn(), toolRow]));

    const tool = state.items.find((item) => item.kind === 'tool');
    expect(tool?.kind === 'tool' ? tool.images : undefined).toEqual([]);
  });

  it('restores stored reasoning so a reload still shows what the model thought', () => {
    // Switching sessions or reloading the page refetches the stored rows; the
    // thinking block the reader watched arrive must come back with them.
    const state = seedChatState(
      sessionWith([
        storedMessage({ id: 1, role: 'user', content: 'Find the login bug.' }),
        storedMessage({
          id: 2,
          role: 'assistant',
          content: 'The bug is in auth.',
          reasoning_content: 'Read the auth module first.',
        }),
      ]),
    );

    const answer = state.items.find((item) => item.kind === 'message' && item.role === 'assistant');
    expect(answer?.kind === 'message' ? answer.reasoning : '').toBe('Read the auth module first.');
    expect(answer?.kind === 'message' ? answer.content : '').toBe('The bug is in auth.');
  });

  it('keeps the reasoning of a tool-calling turn that produced no prose', () => {
    // A reasoning model that thinks and then calls a tool writes an assistant row
    // with `content: ""`. That thought is the only record of the turn, and it has
    // to sit above the tool row it explains after a reload too.
    const state = seedChatState(
      sessionWith([
        storedMessage({
          id: 1,
          role: 'assistant',
          tool_calls: [call],
          reasoning_content: 'Look at the screenshot before answering.',
        }),
      ]),
    );

    expect(state.items.map((item) => item.kind).slice(0, 2)).toEqual(['message', 'tool']);
    const thought = state.items[0];
    expect(thought?.kind === 'message' ? thought.reasoning : '').toBe(
      'Look at the screenshot before answering.',
    );
    expect(thought?.kind === 'message' ? thought.content : 'x').toBe('');
  });

  it('does not render a blank thinking block for whitespace-only reasoning', () => {
    const state = seedChatState(
      sessionWith([
        storedMessage({ id: 1, role: 'assistant', content: 'Done.', reasoning_content: '  \n ' }),
      ]),
    );
    const answer = state.items.find((item) => item.kind === 'message');
    expect(answer?.kind === 'message' ? answer.reasoning : 'x').toBe('');
  });

  it('fills in the agent fields serde omits from a session snapshot', () => {
    // `note`, `progress` and `owned_paths` are all `skip_serializing_if`/`default`
    // on the server, so a snapshot can carry none of them. Components read them
    // as present; the seed is where that is made true.
    const wireAgent = omitFields(
      wireAgentSummary('a_wire', { status: 'running' }),
      'note',
      'progress',
      'owned_paths',
    );
    expect('note' in wireAgent).toBe(false);

    const state = seedChatState({ ...detailFixture, agents: [wireAgent] });

    const [agent] = state.agents;
    expect(agent?.note).toBe('');
    expect(agent?.progress).toBeNull();
    expect(agent?.owned_paths).toEqual([]);
  });

  it('keeps a live iteration count when a refetch returns an older subagent row', () => {
    // A snapshot is written between iterations, so a reseed during a run can
    // carry a count the live progress frames have already passed.
    const live = seedChatState({
      ...detailFixture,
      agents: [wireAgentSummary('a_running', { iterations: 9 })],
    });
    const refetched = seedChatState(
      {
        ...detailFixture,
        agents: [wireAgentSummary('a_running', { iterations: 4 })],
      },
      live,
    );
    expect(refetched.agents[0]?.iterations).toBe(9);
  });

  it('says a reloaded run was cancelled, in the words the live transcript used', () => {
    // The stop is not in the stored messages: the run keeps whatever prose and
    // tool cards it produced, and the live transcript ends them with
    // "Run cancelled." A reload rebuilt the same partial turn and said nothing
    // about why it stopped, so the notice is rebuilt from the settled run row.
    const state = seedChatState({
      ...sessionWith([
        storedMessage({ id: 1, role: 'user', content: 'run the whole suite', run_id: 'r_cancel' }),
        storedMessage({ id: 2, role: 'assistant', content: 'Starting with the parser.', run_id: 'r_cancel' }),
      ]),
      runs: [runRow('cancelled', 'r_cancel', NOW - 60_000, NOW - 50_000)],
    });

    const notice = state.items.find((item) => item.id === 'cancelled:r_cancel');
    expect(notice?.kind).toBe('notice');
    expect(notice?.kind === 'notice' ? notice.level : '').toBe('info');
    expect(notice?.kind === 'notice' ? notice.message : '').toBe('Run cancelled.');
    // The partial answer is still there, above the notice that explains it.
    const answer = state.items.find((item) => item.kind === 'message' && item.role === 'assistant');
    expect(answer?.kind === 'message' ? answer.content : '').toBe('Starting with the parser.');
    expect(state.items.indexOf(notice!)).toBeGreaterThan(state.items.indexOf(answer!));
  });

  it('keeps the live wording for a cancellation even when the row carries an error', () => {
    // Live shows one sentence per cancellation whatever the row says, so a reload
    // must not invent a second kind of message for the same event.
    const state = seedChatState({
      ...detailFixture,
      runs: [runRow('cancelled', 'r_cancel_err', NOW - 60_000, NOW - 50_000, 'stop requested by user')],
    });

    const notice = state.items.find((item) => item.id === 'cancelled:r_cancel_err');
    expect(notice?.kind === 'notice' ? notice.message : '').toBe('Run cancelled.');
    expect(state.items.some((item) => item.kind === 'notice' && item.message.includes('stop requested'))).toBe(false);
  });

  it('explains a reloaded run the server restarted, in the live shape', () => {
    const state = seedChatState({
      ...detailFixture,
      runs: [runRow('interrupted', 'r_interrupted', NOW - 60_000, NOW - 50_000)],
    });

    const notice = state.items.find((item) => item.id === 'interrupted:r_interrupted');
    expect(notice?.kind).toBe('interrupted');
    expect(notice?.kind === 'interrupted' ? notice.message : '').toBe(
      'This run was interrupted — the server restarted while it was working.',
    );
  });

  it('says nothing extra about a run that finished or is still running', () => {
    // Only the three stopped statuses carry a notice; a completed run is its own
    // explanation, and a running one has no ending to describe yet.
    for (const status of ['completed', 'running'] as RunStatus[]) {
      const state = seedChatState({
        ...detailFixture,
        running: status === 'running',
        runs: [runRow(status, `r_${status}`, NOW - 60_000, status === 'running' ? null : NOW - 50_000)],
      });
      expect(
        state.items.filter((item) => item.kind === 'notice' || item.kind === 'interrupted'),
        `a ${status} run must not add a stop notice`,
      ).toEqual([]);
    }
  });

  it('explains a reloaded run that stopped at a limit, from the run row', () => {
    // A run that ends `limit_reached` produces no message of its own, so a
    // reloaded transcript showed the status word and nothing else. The reason the
    // runtime recorded goes back in as the same notice the live run drew.
    const reason =
      'stopped: the last 4 turns produced no new information - every call returned an answer this run already had, or was refused as a repeat.';
    const current = seedChatState({
      ...detailFixture,
      runs: [
        ...detailFixture.runs,
        {
          id: 'r_limit',
          session_id: detailFixture.session.id,
          status: 'limit_reached',
          model: 'gpt-4o-mini',
          prompt: 'keep asking the same question',
          error: reason,
          started_at: 1_710_000_000_000,
          finished_at: 1_710_000_002_000,
        },
      ],
    });

    const notice = current.items.find((item) => item.id === 'limit:r_limit');
    expect(notice?.kind).toBe('notice');
    expect(notice?.kind === 'notice' ? notice.level : '').toBe('warn');
    expect(notice?.kind === 'notice' ? notice.message : '').toBe(reason);

    // A limit run with no recorded reason stays quiet rather than inventing one.
    const bare = seedChatState({
      ...detailFixture,
      runs: [
        ...detailFixture.runs,
        {
          id: 'r_limit_bare',
          session_id: detailFixture.session.id,
          status: 'limit_reached',
          model: 'gpt-4o-mini',
          prompt: 'stop',
          error: null,
          started_at: 1_710_000_000_000,
          finished_at: 1_710_000_001_000,
        },
      ],
    });
    expect(bare.items.some((item) => item.id === 'limit:r_limit_bare')).toBe(false);
  });
});

describe('mergeTranscriptRows', () => {
  const rows = (ids: number[]): StoredMessage[] =>
    ids.map((id) => storedMessage({ id, role: 'user', content: `message ${id}` }));

  it('keeps the pages already loaded and adds the refreshed tail', () => {
    // The tail overlaps the loaded range (id 11 is on both sides), so the union
    // is contiguous and the cursor still points below the oldest loaded row.
    const merged = mergeTranscriptRows(
      { messages: rows([10, 11]), before: 10, truncated: true },
      { messages: rows([11, 12, 13]), messages_before: 11 },
    );

    expect(merged.overlap).toBe(true);
    expect(merged.messages.map((message) => message.id)).toEqual([10, 11, 12, 13]);
    expect(merged.before).toBe(10);
    expect(merged.truncated).toBe(true);
  });

  it('knows the union is complete when the start was already loaded', () => {
    const merged = mergeTranscriptRows(
      { messages: rows([9, 10, 11]), before: null, truncated: false },
      { messages: rows([11, 12, 13]), messages_before: 11 },
    );

    expect(merged.messages.map((message) => message.id)).toEqual([9, 10, 11, 12, 13]);
    expect(merged.before).toBeNull();
    expect(merged.truncated).toBe(false);
  });

  it('takes the fresh tail when the ranges do not touch', () => {
    const merged = mergeTranscriptRows(
      { messages: rows([10, 11]), before: 10, truncated: true },
      { messages: rows([20, 21]), messages_before: 20 },
    );

    expect(merged.overlap).toBe(false);
    expect(merged.messages.map((message) => message.id)).toEqual([20, 21]);
    expect(merged.before).toBe(20);
  });

  it('does not glue an older page in front of a newer loaded range', () => {
    // A rewind left the server with a shorter transcript than the cached pages.
    const merged = mergeTranscriptRows(
      { messages: rows([20, 21]), before: 20, truncated: true },
      { messages: rows([5]), messages_before: null },
    );

    expect(merged.overlap).toBe(false);
    expect(merged.messages.map((message) => message.id)).toEqual([5]);
    expect(merged.before).toBeNull();
    expect(merged.truncated).toBe(false);
  });

  it('keeps the loaded rows when the snapshot has no messages yet', () => {
    const loaded = { messages: rows([1, 2]), before: 1, truncated: true };
    expect(mergeTranscriptRows(loaded, { messages: [] })).toEqual({ ...loaded, overlap: true });
  });
});

/*
 * The ids a live transcript lacks.
 *
 * A run's rows are built from events, which carry run ids and call ids but no
 * stored message id - so the rewind control had nothing to resolve and only
 * appeared after a reload re-read the session. These tests pin the join between
 * the two views: what it attaches, what it refuses to guess at, and - the reason
 * it exists at all - that it keeps every item's own id, because the transcript
 * keys its rows, measured heights and entry animation on those.
 */
describe('withStoredMessageIds', () => {
  /** The stored id a row learned, whatever kind of row it is. */
  const storedId = (item: ChatItem): number | undefined =>
    'messageId' in item ? item.messageId : undefined;

  function live(
    overrides: Partial<import('./chat').MessageItem> = {},
  ): import('./chat').MessageItem {
    return {
      kind: 'message',
      id: 'user:r_x',
      at: NOW,
      role: 'user',
      content: 'fix the login page',
      reasoning: '',
      streaming: false,
      runId: 'r_x',
      agentId: null,
      ...overrides,
    };
  }

  it('names the stored row behind a live prompt and answer', () => {
    const items = [
      live(),
      live({ id: 'stream:r_x:1', role: 'assistant', content: 'Done.' }),
    ];
    const detail = sessionWith([
      storedMessage({ id: 10, role: 'user', content: 'fix the login page' }),
      storedMessage({ id: 11, role: 'assistant', content: 'Done.' }),
    ]);

    expect(withStoredMessageIds(items, detail).map(storedId)).toEqual([10, 11]);
  });

  it('gives repeated prompts their own rows, in the order they were sent', () => {
    // A prompt and a steer that repeats it. Both live items carry the same text,
    // so matching on content alone sent *both* to the newest row - and clicking
    // the first one rewound to the second one's turn.
    const items = [
      live({ id: 'user:r_x', content: 'continue' }),
      live({ id: 'steer:r_x:2', content: 'continue' }),
    ];
    const detail = sessionWith([
      storedMessage({ id: 10, role: 'user', content: 'continue' }),
      storedMessage({ id: 20, role: 'user', content: 'continue' }),
    ]);

    expect(withStoredMessageIds(items, detail).map(storedId)).toEqual([10, 20]);
  });

  it('refuses to guess when there are more live items than stored rows', () => {
    // Three prompts, two rows: the two that match take a row each, and the third
    // gets none - no rewind beats a rewind to somebody else's turn.
    const items = [
      live({ id: 'a', content: 'continue' }),
      live({ id: 'b', content: 'continue' }),
      live({ id: 'c', content: 'continue' }),
    ];
    const detail = sessionWith([
      storedMessage({ id: 10, role: 'user', content: 'continue' }),
      storedMessage({ id: 20, role: 'user', content: 'continue' }),
    ]);

    expect(withStoredMessageIds(items, detail).map(storedId)).toEqual([10, 20, undefined]);
  });

  it('keeps every item id, which is what the transcript keys its rows on', () => {
    const items = [live()];
    const next = withStoredMessageIds(items, sessionWith([storedMessage({ id: 10, role: 'user' })]));

    expect(next[0]!.id).toBe('user:r_x');
    expect(storedId(next[0]!)).toBe(10);
  });

  it('places a tool call by its call id, not by position', () => {
    const items: ChatItem[] = [
      live(),
      {
        kind: 'tool',
        id: 'tool:call_9',
        at: NOW,
        callId: 'call_9',
        name: 'read_file',
        args: { path: 'a.txt' },
        status: 'ok',
        output: '',
        truncated: false,
        durationMs: null,
        runId: 'r_x',
      },
    ];
    const detail = sessionWith([
      storedMessage({ id: 4, role: 'user' }),
      storedMessage({ id: 5, role: 'assistant', tool_calls: [{ id: 'call_9', name: 'read_file', arguments: '{}' }] }),
      storedMessage({ id: 6, role: 'tool', tool_call_id: 'call_9', content: 'ok', name: 'read_file' }),
    ]);

    expect(storedId(withStoredMessageIds(items, detail)[1]!)).toBe(5);
  });

  it('picks the prose row the reader means when a run wrote several', () => {
    // A turn that called a tool wrote an assistant row per iteration; the prose
    // the reader clicked is the one whose text this row carries.
    const items = [live({ id: 'stream:r_x:2', role: 'assistant', content: 'the answer' })];
    const detail = sessionWith([
      storedMessage({ id: 1, role: 'assistant', content: '', reasoning_content: 'thinking' }),
      storedMessage({ id: 2, role: 'assistant', content: 'the answer' }),
    ]);

    expect(storedId(withStoredMessageIds(items, detail)[0]!)).toBe(2);
  });

  it('leaves a row it cannot place without an id rather than guessing', () => {
    const items = [live({ runId: 'r_other' })];
    const detail = sessionWith([storedMessage({ id: 10, role: 'user' })]);

    expect(storedId(withStoredMessageIds(items, detail)[0]!)).toBeUndefined();
  });

  it('returns the same array when nothing changed, so no row re-renders', () => {
    const items = [live({ messageId: 10 })];
    const detail = sessionWith([storedMessage({ id: 10, role: 'user' })]);

    expect(withStoredMessageIds(items, detail)).toBe(items);
  });

  it('ignores a subagent row, whose ids are not this transcript', () => {
    const items = [live()];
    const detail = sessionWith([storedMessage({ id: 10, role: 'user', agent_id: 'a_1' })]);

    expect(storedId(withStoredMessageIds(items, detail)[0]!)).toBeUndefined();
  });
});
